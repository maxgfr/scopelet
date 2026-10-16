use scopelet::{compress, model::*, pipeline, query, recovery, sources, store::Store};
use serde_json::{Value, json};
use std::{
    fs,
    sync::{Arc, atomic::AtomicBool},
};

fn cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}
#[test]
fn rejected_compression_never_opens_a_store() {
    let dir = tempfile::tempdir().unwrap();
    for raw in [
        b"small\r\n".to_vec(),
        vec![255; 5000],
        format!("<persisted-output>{}", "x".repeat(6000)).into_bytes(),
        // Already a view: never compressed again.
        format!(
            "[scopelet v5 000000000000 complete]\n{}",
            "x\n".repeat(3000)
        )
        .into_bytes(),
    ] {
        let path = dir.path().join("must-not-exist");
        assert_eq!(
            &*compress::automatic_lazy(&raw, Some(path.clone()), 4096).unwrap(),
            raw
        );
        assert!(!path.exists());
    }
}

#[test]
fn streaming_serialization_preserves_hashes_and_rejects_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().into())).unwrap();
    let value: Value = serde_json::from_str(
        r#"{"decimal":1.2300,"integer":900719925474099312345,"escape":"\r\n\"é"}"#,
    )
    .unwrap();
    let expected = store
        .put("artifact", &serde_json::to_vec(&value).unwrap())
        .unwrap();
    assert_eq!(store.put_json(&value).unwrap(), expected);
    let path = dir
        .path()
        .join("artifacts")
        .join(expected.split_once(':').unwrap().1);
    let serialized = store.get(&expected).unwrap();
    // A truncated artifact is an interrupted publication: republishing repairs it.
    fs::write(&path, b"corrupted").unwrap();
    assert_eq!(store.put_json(&value).unwrap(), expected);
    assert_eq!(store.get(&expected).unwrap(), serialized);
    // Same-length damage is reused by address and rejected on the read that matters.
    let mut altered = serialized.clone();
    altered[0] ^= 1;
    fs::write(&path, &altered).unwrap();
    assert_eq!(store.put_json(&value).unwrap(), expected);
    assert!(
        store
            .get(&expected)
            .unwrap_err()
            .to_string()
            .contains("integrity")
    );
}

#[test]
fn distinct_late_diagnostics_survive_repeated_early_errors() {
    let dir = tempfile::tempdir().unwrap();
    let raw = format!(
        "start\n{}fatal error: rare root cause\nend\n",
        "error: retry failed\nworking\n".repeat(1000)
    );
    let out = compress::automatic_lazy(raw.as_bytes(), Some(dir.path().into()), 1024).unwrap();
    assert!(
        std::str::from_utf8(&out)
            .unwrap()
            .contains("rare root cause")
    );
    assert!(out.len() <= 1024);
}

#[test]
fn optimized_queries_equal_materialized_datasets() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().join("cache"))).unwrap();
    let file = dir.path().join("rows.jsonl");
    fs::write(&file, "{\"id\":9007199254740993,\"state\":\"bad\",\"group\":null}\n\n{\"state\":\"ok\"}\n{\"state\":\"bad\",\"group\":1.2300}\n").unwrap();
    for operations in [
        json!([{"op":"count"}]),
        json!([{"op":"filter","pointer":"/state","equals":"bad"},{"op":"count"}]),
        json!([{"op":"filter","pointer":"/state","equals":"bad"},{"op":"group","pointer":"/group"}]),
        json!([{"op":"project","pointers":["/state"]},{"op":"count"}]),
        json!([{"op":"filter","pointer":"/missing","equals":null},{"op":"count"}]),
    ] {
        let request: Request = serde_json::from_value(json!({"version":1,"source":{"type":"file","path":file,"format":"jsonl"},"operations":operations})).unwrap();
        let mut old = sources::load(&request.source, &store, cancel()).unwrap();
        pipeline::apply(&mut old, &request.operations).unwrap();
        let new = query::execute(&request, &store, cancel()).unwrap();
        assert_eq!(
            serde_json::to_vec(&old).unwrap(),
            serde_json::to_vec(&new).unwrap()
        );
    }
}

#[test]
fn malformed_last_row_precedes_group_error_and_emits_no_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().join("cache"))).unwrap();
    let file = dir.path().join("bad.jsonl");
    fs::write(&file, "{\"other\":1}\n{broken\n").unwrap();
    let request: Request = serde_json::from_value(json!({"version":1,"source":{"type":"file","path":file,"format":"jsonl"},"operations":[{"op":"group","pointer":"/missing"}]})).unwrap();
    let error = query::execute(&request, &store, cancel()).unwrap_err();
    assert!(format!("{error:#}").contains("malformed JSONL"));
    assert_eq!(
        fs::read_dir(store.root.join("artifacts")).unwrap().count(),
        2
    );
}

#[test]
fn repository_search_pushdown_preserves_order_snapshots_and_counts() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().join("cache"))).unwrap();
    fs::write(dir.path().join("a.txt"), "first\nnoise\nsecond\r\n").unwrap();
    fs::write(dir.path().join("b.txt"), "decoy\n").unwrap();
    fs::write(dir.path().join("c.txt"), "first\nsecond\n").unwrap();
    let request: Request = serde_json::from_value(json!({"version":1,"source":{"type":"repo","path":dir.path(),"include":["*.txt"]},"operations":[{"op":"search","patterns":["first","second"],"all":true,"context":0},{"op":"count"}]})).unwrap();
    let mut old = sources::load(&request.source, &store, cancel()).unwrap();
    pipeline::apply(&mut old, &request.operations).unwrap();
    let new = query::execute(&request, &store, cancel()).unwrap();
    assert_eq!(
        serde_json::to_vec(&old).unwrap(),
        serde_json::to_vec(&new).unwrap()
    );
}

#[test]
fn recovery_searches_originals_after_source_changes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().join("cache"))).unwrap();
    let file = dir.path().join("old.txt");
    fs::write(&file, "head\r\nold evidence é\r\ntail\r\n").unwrap();
    let mut data = sources::load(
        &Source::File {
            path: file.to_string_lossy().into(),
            format: Format::Text,
        },
        &store,
        cancel(),
    )
    .unwrap();
    pipeline::apply(&mut data, &[Operation::Read { start: 1, end: 1 }]).unwrap();
    let id = store.put_json(&data).unwrap();
    fs::write(&file, "replacement").unwrap();
    assert!(sources::load(&Source::Artifact { id: id.clone() }, &store, cancel()).is_err());
    let recovered =
        recovery::search(&store, &id, &["evidence".into()], (false, false), 0, None).unwrap();
    assert_eq!(recovered.records[0].text, "old evidence é\r\n");
    assert_eq!(recovered.records[0].start_line, Some(2));
    assert!(
        recovery::search(
            &store,
            &id,
            &["evidence".into()],
            (false, false),
            0,
            Some("absent")
        )
        .is_err()
    );
}

#[test]
fn disposable_line_index_recovers_exact_bytes_and_survives_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().into())).unwrap();
    let raw = "a\r\nbé\r\nc\n".as_bytes();
    let id = store.put("blob", raw).unwrap();
    for _ in 0..2 {
        let data = recovery::range(&store, &id, 2, 2).unwrap();
        assert_eq!(data.records[0].text, "bé\r\n");
        let path = dir
            .path()
            .join("line-index-v1")
            .join(id.split_once(':').unwrap().1);
        assert!(path.is_file());
        fs::write(path, b"broken").unwrap();
    }
    assert!(
        recovery::range(&store, &id, 10, 12)
            .unwrap()
            .records
            .is_empty()
    );
    store.clean(0).unwrap();
    assert_eq!(
        fs::read_dir(dir.path().join("line-index-v1"))
            .unwrap()
            .count(),
        2
    );
}

#[test]
fn cli_recovery_flags_work_and_conflicts_fail() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().into())).unwrap();
    let blob = store.put("blob", b"a\nneedle\nz\n").unwrap();
    let mut cmd = assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("scopelet"));
    let out = cmd
        .args([
            "--cache-dir",
            dir.path().to_str().unwrap(),
            "expand",
            &blob,
            "--find",
            "needle",
            "--context",
            "0",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["records"][0]["text"], "needle\n");
    assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("scopelet"))
        .args(["expand", &blob, "--find", "x", "--raw"])
        .assert()
        .code(2);
}

#[test]
fn forged_index_cannot_relabel_source_lines() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().into())).unwrap();
    let raw = b"first\nsecond\nthird\n";
    let id = store.put("blob", raw).unwrap();
    recovery::range(&store, &id, 2, 2).unwrap();
    let path = dir
        .path()
        .join("line-index-v1")
        .join(id.split_once(':').unwrap().1);
    let offsets = vec![0usize, 13, 19];
    let checksum = scopelet::store::digest(&serde_json::to_vec(&(&id, &offsets)).unwrap());
    fs::write(
        &path,
        serde_json::to_vec(&json!({"blob":id,"offsets":offsets,"checksum":checksum})).unwrap(),
    )
    .unwrap();
    assert_eq!(
        recovery::range(&store, &id, 2, 2).unwrap().records[0].text,
        "second\n"
    );
}

#[test]
fn supported_command_profiles_are_bounded_and_noninteractive() {
    use scopelet::commands::profile;
    for command in [
        "rg needle .",
        "grep -n needle file",
        "git --no-pager diff",
        "git log --oneline",
        "go test ./...",
        "node --test check.mjs",
        "npm run lint",
        "pnpm build",
        "yarn typecheck",
        "git log -p",
        "cargo fmt --check",
        "python3 -m pytest",
    ] {
        let args = command
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert!(profile(&args).is_some(), "{command}");
    }
    for command in [
        "git --paginate log",
        "node --test --watch",
        "npm test -- --watch=true",
        "python3 script.py -i",
        "rg --pre=command needle",
        "scopelet run --auto -- npm test",
        "git -p log",
        "find . -delete",
        "sed -i s/a/b/ file",
    ] {
        let args = command
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert!(profile(&args).is_none(), "{command}");
    }
}

#[test]
fn invalid_environment_keeps_hooks_fail_open_and_cleanup_available() {
    let dir = tempfile::tempdir().unwrap();
    let event = json!({"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"cargo test"}});
    // No configuration root (HOME unset), then an unreadable mode file.
    let corrupt = dir.path().join("config");
    fs::create_dir(&corrupt).unwrap();
    fs::write(corrupt.join("mode.json"), b"not json").unwrap();
    let environments: [&dyn Fn(&mut assert_cmd::Command); 2] = [
        &|command| {
            command
                .env_remove("HOME")
                .env_remove("XDG_CONFIG_HOME")
                .env_remove("SCOPELET_CONFIG_DIR");
        },
        &|command| {
            command.env("SCOPELET_CONFIG_DIR", &corrupt);
        },
    ];
    for environment in environments {
        let mut command = assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("scopelet"));
        environment(&mut command);
        let out = command
            .args(["hook", "codex"])
            .write_stdin(event.to_string())
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&out.stdout).unwrap(),
            json!({})
        );
        let mut command = assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("scopelet"));
        environment(&mut command);
        command
            .args([
                "--cache-dir",
                dir.path().join("cache").to_str().unwrap(),
                "clean",
            ])
            .assert()
            .success();
    }
}

#[test]
fn range_recovery_validates_reference_and_cleanup_preserves_foreign_index_files() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().into())).unwrap();
    assert!(recovery::range(&store, "blob:../../outside", 1, 1).is_err());
    let blob = store.put("blob", b"a\nb\n").unwrap();
    recovery::range(&store, &blob, 1, 1).unwrap();
    let folder = dir.path().join("line-index-v1");
    fs::write(folder.join(".scopelet-write-AbCd12345678"), b"ours").unwrap();
    fs::write(folder.join(".tmp-foreign"), b"foreign").unwrap();
    store.clean(0).unwrap();
    assert!(!folder.join(".scopelet-write-AbCd12345678").exists());
    assert_eq!(fs::read(folder.join(".tmp-foreign")).unwrap(), b"foreign");
}

#[cfg(unix)]
#[test]
fn existing_json_artifact_can_be_reused_in_a_readonly_directory() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().into())).unwrap();
    let value = json!({"exact":"é\r\n","n":9007199254740993u64});
    let id = store
        .put("artifact", &serde_json::to_vec(&value).unwrap())
        .unwrap();
    let folder = dir.path().join("artifacts");
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o555)).unwrap();
    let result = store.put_json(&value);
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(result.unwrap(), id);
}

#[cfg(unix)]
#[test]
fn standalone_compression_preserves_interruption_exit_status() {
    use std::{
        io::Write,
        process::{Command, Stdio},
        time::Duration,
    };
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(assert_cmd::cargo::cargo_bin!("scopelet"))
        .args(["--cache-dir", dir.path().to_str().unwrap(), "compress"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    // Writing beyond pipe capacity proves the child is reading after handler setup.
    input.write_all(&b"working\n".repeat(40000)).unwrap();
    unsafe {
        libc::kill(child.id() as i32, libc::SIGINT);
    }
    std::thread::sleep(Duration::from_millis(30));
    drop(input);
    assert_eq!(child.wait().unwrap().code(), Some(130));
}
