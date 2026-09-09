use assert_cmd::Command;
use scopelet::{compress, store::Store};
use serde_json::{Value, json};
use std::fs;

fn cli(dir: &std::path::Path) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("scopelet"));
    command
        .env("SCOPELET_CONFIG_DIR", dir.join("config"))
        .env("SCOPELET_CACHE_DIR", dir.join("cache"))
        .env("CODEX_HOME", dir.join("codex"))
        .env("CLAUDE_CONFIG_DIR", dir.join("claude"));
    command
}
#[test]
fn compression_keeps_middle_error_and_original_crlf_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().join("cache"))).unwrap();
    let raw = format!(
        "{}AssertionError: expected 9007199254740993, got 0\r\n{}",
        "working\r\n".repeat(500),
        "working\r\n".repeat(500)
    );
    let output = compress::automatic(raw.as_bytes(), &store, 4096).unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("AssertionError: expected 9007199254740993, got 0\r\n"));
    assert!(text.contains("repeat=500"));
    let artifact = text
        .split_whitespace()
        .find(|s| s.starts_with("artifact:"))
        .unwrap();
    let data: Value = serde_json::from_slice(&store.get(artifact).unwrap()).unwrap();
    assert_eq!(
        store
            .get(data["snapshots"][0]["blob"].as_str().unwrap())
            .unwrap(),
        raw.as_bytes()
    );
}
#[test]
fn small_binary_and_already_persisted_outputs_stay_exact() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().join("cache"))).unwrap();
    for bytes in [
        b"hello\r\n".to_vec(),
        vec![255; 5000],
        format!("<persisted-output>{}", "a".repeat(5000)).into_bytes(),
    ] {
        assert_eq!(compress::automatic(&bytes, &store, 4096).unwrap(), bytes);
    }
}
#[test]
fn structured_selection_keeps_whole_records_and_marks_omissions() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().join("cache"))).unwrap();
    let rows: Vec<_> = (0..100)
        .map(|i| json!({"id":i,"value":"x".repeat(100),"state":if i==50 {"failed"} else {"ok"}}))
        .collect();
    let raw = serde_json::to_vec(&rows).unwrap();
    let text = String::from_utf8(compress::automatic(&raw, &store, 4096).unwrap()).unwrap();
    assert!(text.contains("\"id\":50"));
    assert!(text.contains("display_complete=false"));
    for line in text
        .lines()
        .filter(|l| l.starts_with("input"))
        .map(|l| l.split_once(": ").unwrap().1)
    {
        let row: Value = serde_json::from_str(line).unwrap();
        assert!(rows.contains(&row));
    }
}
#[test]
fn giant_line_stays_native_when_no_complete_evidence_unit_fits() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().join("cache"))).unwrap();
    let raw = "é".repeat(5000);
    let output = compress::automatic(raw.as_bytes(), &store, 4096).unwrap();
    let text = String::from_utf8(output).unwrap();
    assert_eq!(text, raw);
}
#[test]
fn auto_runs_once_preserves_streams_status_and_survives_bad_cache() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("marker");
    fs::write(dir.path().join("cache"), b"not a directory").unwrap();
    let script = format!(
        "printf x >> '{}'; printf 'out\\r\\n'; printf 'err\\n' >&2; exit 7",
        marker.display()
    );
    let out = cli(dir.path())
        .args(["run", "--auto", "--", "sh", "-c", &script])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(7));
    assert_eq!(out.stdout, b"out\r\n");
    assert_eq!(out.stderr, b"err\n");
    assert_eq!(fs::read(marker).unwrap(), b"x");
}
#[test]
fn install_preserves_other_hooks_is_idempotent_and_uninstall_removes_only_ours() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("codex")).unwrap();
    let path = dir.path().join("codex/hooks.json");
    let other = json!({"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"other-policy"}]}]},"custom":7});
    fs::write(&path, serde_json::to_vec(&other).unwrap()).unwrap();
    cli(dir.path())
        .args(["install", "--agent", "codex"])
        .assert()
        .success();
    let installed = fs::read(&path).unwrap();
    cli(dir.path())
        .args(["install", "--agent", "codex"])
        .assert()
        .success();
    assert_eq!(fs::read(&path).unwrap(), installed);
    let value: Value = serde_json::from_slice(&installed).unwrap();
    assert_eq!(
        value["hooks"]["PreToolUse"][0],
        other["hooks"]["PreToolUse"][0]
    );
    cli(dir.path())
        .args(["uninstall", "--agent", "codex"])
        .assert()
        .success();
    let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(value, other);
}
fn hook(dir: &std::path::Path, agent: &str, event: Value) -> Value {
    let result = cli(dir)
        .args(["hook", agent])
        .write_stdin(event.to_string())
        .output()
        .unwrap();
    assert!(result.status.success());
    serde_json::from_slice(&result.stdout).unwrap()
}
#[test]
fn codex_rewrites_only_simple_commands_and_off_disables_it() {
    let dir = tempfile::tempdir().unwrap();
    let event = json!({"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"python3 checks.py","description":"test","timeout":1000},"permission_mode":"default"});
    let value = hook(dir.path(), "codex", event.clone());
    let input = &value["hookSpecificOutput"]["updatedInput"];
    assert!(input["command"].as_str().unwrap().contains("run --auto"));
    assert_eq!(input["timeout"], 1000);
    assert_eq!(input["description"], "test");
    assert!(
        value["hookSpecificOutput"]
            .get("updatedPermissions")
            .is_none()
    );
    for command in [
        "echo x | cat",
        "python3 checks.py; touch x",
        "cat $(secret)",
        "scopelet run -- cargo test",
        "rtk cargo test",
        "npm test -- --watch",
        "python3 -c 'print(1)'",
        "cat *.rs",
    ] {
        let mut event = event.clone();
        event["tool_input"]["command"] = json!(command);
        assert_eq!(hook(dir.path(), "codex", event), json!({}));
    }
    cli(dir.path()).args(["mode", "off"]).assert().success();
    assert_eq!(hook(dir.path(), "codex", event), json!({}));
}
#[test]
fn claude_replacement_preserves_envelope_and_small_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let mut event = json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_response":{"stdout":"noise\n".repeat(2000),"stderr":"warning: keep me\n","interrupted":false,"isImage":false,"exit_code":7}});
    let out = hook(dir.path(), "claude", event.clone());
    let replacement = &out["hookSpecificOutput"]["updatedToolOutput"];
    assert_eq!(replacement["stderr"], event["tool_response"]["stderr"]);
    assert_eq!(replacement["exit_code"], 7);
    assert_eq!(replacement["interrupted"], false);
    event["tool_response"]["stdout"] = json!("small\n");
    assert_eq!(hook(dir.path(), "claude", event), json!({}));
}
#[test]
fn context_is_sent_only_at_start_or_mode_change() {
    let dir = tempfile::tempdir().unwrap();
    let event = json!({"hook_event_name":"UserPromptSubmit","session_id":"session-one"});
    assert_ne!(hook(dir.path(), "codex", event.clone()), json!({}));
    assert_eq!(hook(dir.path(), "codex", event.clone()), json!({}));
    cli(dir.path()).args(["mode", "caveman"]).assert().success();
    assert!(
        hook(dir.path(), "codex", event)["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("telegraphic")
    );
}
#[test]
fn query_shortcuts_compute_exact_groups() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    fs::write(
        &path,
        "{\"state\":\"failed\",\"suite\":\"a\"}\n{\"state\":\"ok\",\"suite\":\"b\"}\n",
    )
    .unwrap();
    let out = cli(dir.path())
        .args([
            "query",
            "--file",
            path.to_str().unwrap(),
            "--format",
            "jsonl",
            "--filter",
            "/state",
            "--equals",
            "\"failed\"",
            "--group",
            "/suite",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["records"][0]["value"], json!({"key":"a","count":1}));
}

#[test]
fn factored_json_preserves_every_cell_and_large_number() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().join("cache"))).unwrap();
    let rows:Vec<Value>=(0..60).map(|i| serde_json::from_str(&format!(r#"{{"long_identifier_column":{i},"nullable_annotation_column":null,"exact_quantity_column":900719925474099312345}}"#)).unwrap()).collect();
    let raw = serde_json::to_vec(&rows).unwrap();
    let output = String::from_utf8(compress::automatic(&raw, &store, 4096).unwrap()).unwrap();
    let table: Value = serde_json::from_str(
        output
            .lines()
            .find_map(|l| l.strip_prefix("table "))
            .unwrap(),
    )
    .unwrap();
    for (index, row) in table["rows"].as_array().unwrap().iter().enumerate() {
        let reconstructed: serde_json::Map<String, Value> = table["columns"]
            .as_array()
            .unwrap()
            .iter()
            .zip(row.as_array().unwrap())
            .map(|(key, value)| (key.as_str().unwrap().into(), value.clone()))
            .collect();
        assert_eq!(Value::Object(reconstructed), rows[index]);
    }
    assert_eq!(table["rows"].as_array().unwrap().len(), rows.len());
    assert!(output.contains("display_complete=true"));
}

#[test]
fn compact_run_respects_minimum_budget_and_exit_status() {
    let dir = tempfile::tempdir().unwrap();
    let out = cli(dir.path())
        .args([
            "run",
            "--output",
            "compact",
            "--max-bytes",
            "1024",
            "--",
            "sh",
            "-c",
            "echo diagnostic >&2; exit 7",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(7));
    assert!(out.stdout.len() <= 1024);
    assert!(String::from_utf8_lossy(&out.stdout).contains("diagnostic"));
    let event = json!({"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"python3 checks.py","tty":true}});
    assert_eq!(hook(dir.path(), "codex", event), json!({}));
}

#[test]
fn recognized_and_lists_preserve_short_circuiting_and_status() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("checks.py"),
        "import sys\nprint('failed')\nsys.exit(7)\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("acceptance.py"),
        "from pathlib import Path\nPath('must-not-run').write_text('bad')\n",
    )
    .unwrap();
    let event = json!({"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"python3 checks.py && python3 acceptance.py"}});
    let result = hook(dir.path(), "codex", event);
    let command = result["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .unwrap();
    let out = std::process::Command::new("/bin/sh")
        .args(["-c", command])
        .current_dir(dir.path())
        .env("SCOPELET_CACHE_DIR", dir.path().join("cache"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(7));
    assert_eq!(out.stdout, b"failed\n");
    assert!(!dir.path().join("must-not-run").exists());
    for command in [
        "python3 checks.py && touch marker",
        "python3 checks.py || python3 acceptance.py",
        "cat 'a && b'",
        "python3 checks.py & python3 acceptance.py",
    ] {
        assert_eq!(
            hook(
                dir.path(),
                "codex",
                json!({"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":command}})
            ),
            json!({})
        );
    }
}

#[test]
fn crowded_diagnostics_keep_first_and_final_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().join("cache"))).unwrap();
    let middle = (0..1000)
        .map(|i| format!("error: case {i} failed\n"))
        .collect::<String>();
    let raw = format!("running 1000 tests\n{middle}final status: 1000 failed; exit=7\n");
    let text =
        String::from_utf8(compress::automatic(raw.as_bytes(), &store, 1024).unwrap()).unwrap();
    assert!(text.contains("running 1000 tests"));
    assert!(text.contains("final status: 1000 failed; exit=7"));
    assert!(text.contains("display_complete=false"));
    assert!(text.len() <= 1024);
}
