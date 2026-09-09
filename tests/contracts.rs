use assert_cmd::Command;
use scopelet::{model::*, pipeline, sources, store::Store};
use serde_json::{Value, json};
use std::fs;
use std::sync::{Arc, atomic::AtomicBool};

fn cli() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("scopelet"))
}

#[test]
fn multiline_and_single_line_patterns_keep_all_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().join("cache"))).unwrap();
    let mut data = Dataset::default();
    let text = b"single\nnoise\nstart\nfinish\n";
    sources::ingest(&mut data, &store, "f", text.to_vec(), None, Format::Text).unwrap();
    pipeline::apply(
        &mut data,
        &[Operation::Search {
            patterns: vec!["single".into(), "start\\nfinish".into()],
            all: true,
            regex: true,
            context: 0,
        }],
    )
    .unwrap();
    assert!(
        data.records
            .iter()
            .any(|r| r.text.contains("start\nfinish"))
    );
    assert!(data.records.iter().any(|r| r.text.contains("single")));
}

#[test]
fn invalid_run_budget_does_not_execute_command() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("must-not-exist");
    cli()
        .args([
            "--cache-dir",
            dir.path().to_str().unwrap(),
            "run",
            "--max-bytes",
            "1",
            "--",
            "touch",
            marker.to_str().unwrap(),
        ])
        .assert()
        .code(2);
    assert!(!marker.exists());
}

#[test]
fn run_envelope_fits_budget_and_keeps_late_error() {
    let dir = tempfile::tempdir().unwrap();
    let out = cli()
        .args([
            "--cache-dir",
            dir.path().to_str().unwrap(),
            "run",
            "--max-bytes",
            "1024",
            "--",
            "sh",
            "-c",
            "echo progress; echo late-failure >&2; exit 7",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(7));
    assert!(out.stdout.len() <= 1024, "{} bytes", out.stdout.len());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["exit_code"], 7);
    assert!(String::from_utf8_lossy(&out.stdout).contains("late-failure"));
}

#[test]
fn source_line_ranges_remain_absolute_and_byte_exact() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().join("cache"))).unwrap();
    let mut data = Dataset::default();
    sources::ingest(
        &mut data,
        &store,
        "f",
        "a\r\nbé\r\nc\r\nd\r\n".as_bytes().to_vec(),
        None,
        Format::Text,
    )
    .unwrap();
    pipeline::apply(
        &mut data,
        &[
            Operation::Read { start: 2, end: 4 },
            Operation::Read { start: 3, end: 3 },
        ],
    )
    .unwrap();
    assert_eq!(data.records[0].text, "c\r\n");
    assert_eq!(data.records[0].start_line, Some(3));
}

#[test]
fn repository_scope_applies_ignores_globs_and_binary_policy() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
    fs::write(dir.path().join("ignored.txt"), "secret").unwrap();
    fs::write(dir.path().join("src/a.rs"), "target").unwrap();
    fs::write(dir.path().join("src/b.rs"), "skip").unwrap();
    fs::write(dir.path().join("src/raw.bin"), [0, 1, 2]).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let store = Store::open(Some(cache.path().to_path_buf())).unwrap();
    let data = sources::load(
        &Source::Repo {
            path: dir.path().to_str().unwrap().into(),
            include: vec!["src/**".into()],
            exclude: vec!["src/b.rs".into()],
        },
        &store,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    assert_eq!(data.records.len(), 1);
    assert_eq!(data.records[0].text, "target");
    assert_eq!(data.skipped.get("binary_or_non_utf8"), Some(&1));
}

#[test]
fn corrupt_snapshot_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().to_path_buf())).unwrap();
    let id = store.put("blob", b"original").unwrap();
    fs::write(
        dir.path().join("blobs").join(id.split(':').nth(1).unwrap()),
        b"altered",
    )
    .unwrap();
    assert!(
        store
            .get(&id)
            .unwrap_err()
            .to_string()
            .contains("integrity")
    );
}

#[test]
fn empty_results_are_complete_with_a_saved_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("x.json");
    fs::write(&file, "[]").unwrap();
    let output = cli()
        .args([
            "--cache-dir",
            dir.path().join("cache").to_str().unwrap(),
            "query",
            "--file",
            file.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["total_records"], 0);
    assert_eq!(v["scan_complete"], true);
    assert_eq!(v["display_complete"], true);
    assert!(v["artifact"].as_str().unwrap().starts_with("artifact:"));
}

#[test]
fn unknown_request_fields_fail_instead_of_being_ignored() {
    let dir = tempfile::tempdir().unwrap();
    cli()
        .args([
            "--cache-dir",
            dir.path().to_str().unwrap(),
            "query",
            "--spec",
            "-",
        ])
        .write_stdin(
            json!({"version":1,"source":{"type":"file","path":"not-opened","formatt":"json"}})
                .to_string(),
        )
        .assert()
        .code(2);
}

#[test]
fn oversized_record_has_explicit_recovery_instead_of_stuck_pagination() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().to_path_buf())).unwrap();
    let mut data = Dataset::default();
    sources::ingest(
        &mut data,
        &store,
        "large",
        vec![b'x'; 4000],
        None,
        Format::Text,
    )
    .unwrap();
    let view = scopelet::render::render(&data, &store, Mode::Default, 1024, 0).unwrap();
    assert_eq!(view.blocked_record, Some(0));
    assert_eq!(view.next_offset, None);
    assert!(!view.display_complete);
    let recovered: Dataset = serde_json::from_slice(&store.get(&view.artifact).unwrap()).unwrap();
    assert_eq!(recovered.records[0].text.len(), 4000);
}

#[test]
fn regex_line_anchors_preserve_crlf_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().to_path_buf())).unwrap();
    let mut data = Dataset::default();
    sources::ingest(
        &mut data,
        &store,
        "f",
        b"noise\r\nexact\r\nother\r\n".to_vec(),
        None,
        Format::Text,
    )
    .unwrap();
    pipeline::apply(
        &mut data,
        &[Operation::Search {
            patterns: vec!["^exact$".into()],
            all: false,
            regex: true,
            context: 0,
        }],
    )
    .unwrap();
    assert_eq!(data.records[0].text, "exact\r\n");
    assert_eq!(data.records[0].start_line, Some(2));
}

#[test]
fn cleanup_retains_old_blob_referenced_by_recent_artifact() {
    use std::time::{Duration, SystemTime};
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().to_path_buf())).unwrap();
    let mut data = Dataset::default();
    sources::ingest(
        &mut data,
        &store,
        "f",
        b"original".to_vec(),
        None,
        Format::Text,
    )
    .unwrap();
    let view = scopelet::render::render(&data, &store, Mode::Default, 4096, 0).unwrap();
    let blob = data.snapshots[0].blob.clone();
    let path = dir
        .path()
        .join("blobs")
        .join(blob.split(':').nth(1).unwrap());
    fs::File::open(path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(86400 * 20))
        .unwrap();
    assert_eq!(store.clean(7).unwrap(), 0);
    assert_eq!(store.get(&blob).unwrap(), b"original");
    assert!(store.get(&view.artifact).is_ok());
}

#[cfg(unix)]
#[test]
fn opening_existing_cache_does_not_change_user_directory_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    Store::open(Some(dir.path().to_path_buf())).unwrap();
    assert_eq!(
        fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[test]
fn run_parse_failure_preserves_successful_command_status_and_raw_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let out = cli()
        .args([
            "--cache-dir",
            dir.path().to_str().unwrap(),
            "run",
            "--format",
            "json",
            "--",
            "printf",
            "invalid-json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let view: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(view["parse_error"].is_string());
    let store = Store::open(Some(dir.path().to_path_buf())).unwrap();
    assert_eq!(
        store.get(view["stdout"].as_str().unwrap()).unwrap(),
        b"invalid-json"
    );
}

#[test]
fn newline_heavy_command_has_bounded_record_overhead_and_exact_original() {
    let dir = tempfile::tempdir().unwrap();
    let out = cli()
        .args([
            "--cache-dir",
            dir.path().to_str().unwrap(),
            "run",
            "--",
            "sh",
            "-c",
            "head -c 240000 /dev/zero | tr '\\000' '\\n'",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let view: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(view["result"]["total_records"].as_u64().unwrap() <= 10_000);
    let store = Store::open(Some(dir.path().to_path_buf())).unwrap();
    assert_eq!(
        store.get(view["stdout"].as_str().unwrap()).unwrap(),
        vec![b'\n'; 240000]
    );
}

#[test]
fn expand_manifest_stays_inside_its_byte_budget() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    fs::create_dir(&repo).unwrap();
    for i in 0..80 {
        fs::write(
            repo.join(format!("module_with_a_fairly_long_name_{i:03}.txt")),
            format!("value {i}\n"),
        )
        .unwrap();
    }
    let cache = dir.path().join("cache");
    let out = cli()
        .args([
            "--cache-dir",
            cache.to_str().unwrap(),
            "query",
            "--repo",
            repo.to_str().unwrap(),
            "--count",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let artifact = serde_json::from_slice::<Value>(&out).unwrap()["artifact"]
        .as_str()
        .unwrap()
        .to_string();

    let manifest = cli()
        .args([
            "--cache-dir",
            cache.to_str().unwrap(),
            "expand",
            &artifact,
            "--manifest",
            "--max-bytes",
            "2048",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    assert!(
        manifest.len() <= 2048,
        "manifest emitted {} bytes over a 2048 byte budget",
        manifest.len()
    );
    let value: Value = serde_json::from_slice(&manifest).unwrap();
    assert_eq!(value["total_snapshots"], json!(80));
    assert!(value["shown_snapshots"].as_u64().unwrap() < 80);
    assert!(value.get("records").is_none());
    assert!(
        value["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|note| note.as_str().unwrap().contains("Manifest truncated"))
    );
}

#[test]
fn spec_query_rejects_flags_it_would_ignore() {
    let dir = tempfile::tempdir().unwrap();
    let spec = dir.path().join("spec.json");
    let target = dir.path().join("data.txt");
    fs::write(&target, "one\ntwo\n").unwrap();
    fs::write(
        &spec,
        json!({
            "version": 1,
            "source": {"type": "file", "path": target.to_str().unwrap()},
            "operations": [{"op": "count"}]
        })
        .to_string(),
    )
    .unwrap();
    let cache = dir.path().join("cache");
    let base = [
        "--cache-dir",
        cache.to_str().unwrap(),
        "query",
        "--spec",
        spec.to_str().unwrap(),
    ];

    cli().args(base).assert().success();
    for ignored in [
        vec!["--count"],
        vec!["--context", "9"],
        vec!["--format", "json"],
    ] {
        let output = cli()
            .args(base)
            .args(&ignored)
            .assert()
            .failure()
            .get_output()
            .clone();
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("cannot be used with"),
            "{ignored:?} should be rejected next to --spec"
        );
    }
}

#[test]
fn expanding_an_artifact_keeps_it_out_of_age_based_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("data.txt");
    fs::write(&target, "alpha\nbeta\n").unwrap();
    let cache = dir.path().join("cache");
    let out = cli()
        .args([
            "--cache-dir",
            cache.to_str().unwrap(),
            "query",
            "--file",
            target.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let artifact = serde_json::from_slice::<Value>(&out).unwrap()["artifact"]
        .as_str()
        .unwrap()
        .to_string();
    let stored = cache
        .join("artifacts")
        .join(artifact.split_once(':').unwrap().1);
    let stale = std::time::SystemTime::now() - std::time::Duration::from_secs(10 * 86400);
    fs::File::options()
        .write(true)
        .open(&stored)
        .unwrap()
        .set_modified(stale)
        .unwrap();

    cli()
        .args(["--cache-dir", cache.to_str().unwrap(), "expand", &artifact])
        .assert()
        .success();

    let removed = cli()
        .args([
            "--cache-dir",
            cache.to_str().unwrap(),
            "clean",
            "--older-days",
            "7",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&removed).unwrap()["removed"],
        json!(0),
        "a just-expanded artifact must survive cleanup"
    );
}
