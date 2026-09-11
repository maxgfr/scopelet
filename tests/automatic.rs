use assert_cmd::Command;
use scopelet::{compress, store::Store};
use serde_json::{Value, json};
use std::fs;

fn cli(dir: &std::path::Path) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("scopelet"));
    command
        .env("SCOPELET_COMPACT_VERSION", "1")
        .env("SCOPELET_CONFIG_DIR", dir.join("config"))
        .env("SCOPELET_CACHE_DIR", dir.join("cache"))
        .env("CODEX_HOME", dir.join("codex"))
        .env("CLAUDE_CONFIG_DIR", dir.join("claude"))
        .env("OPENCODE_CONFIG_DIR", dir.join("opencode"));
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
fn claude_persistence_metadata_bypasses_compression_before_preview_rendering() {
    let dir = tempfile::tempdir().unwrap();
    let event = json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_response":{"stdout":"noise\n".repeat(2000),"stderr":"","interrupted":false,"isImage":false}});
    for metadata in [
        json!({"persistedOutputPath":"/synthetic/tool-results/checks.txt"}),
        json!({"persistedOutputSize":12000}),
    ] {
        let mut persisted = event.clone();
        persisted["tool_response"]
            .as_object_mut()
            .unwrap()
            .extend(metadata.as_object().unwrap().clone());
        assert_eq!(hook(dir.path(), "claude", persisted), json!({}));
        assert!(!dir.path().join("cache").exists());
    }
    let mut inline = event;
    inline["tool_response"]["persistedOutputPath"] = json!(null);
    inline["tool_response"]["persistedOutputSize"] = json!(0);
    assert!(
        hook(dir.path(), "claude", inline)["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
            .as_str()
            .unwrap()
            .contains("[scopelet compact-v1")
    );
}
#[test]
fn small_automatic_outputs_do_not_open_cache() {
    let dir = tempfile::tempdir().unwrap();
    let event = json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_response":{"stdout":"x".repeat(2048),"stderr":"warning\r\n","interrupted":false,"isImage":false}});
    assert_eq!(hook(dir.path(), "claude", event), json!({}));
    assert!(!dir.path().join("cache").exists());
    cli(dir.path())
        .args([
            "run",
            "--auto",
            "--",
            "sh",
            "-c",
            "printf 'ok\\r\\n'; printf 'warning\\n' >&2; exit 7",
        ])
        .assert()
        .code(7)
        .stdout("ok\r\n")
        .stderr("warning\n");
    assert!(!dir.path().join("cache").exists());
}
#[test]
fn codex_small_file_reads_stay_native_and_large_reads_are_wrapped() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("small.txt");
    fs::write(&file, vec![b'x'; 2048]).unwrap();
    let mut event = json!({"hook_event_name":"PreToolUse","tool_name":"Bash","cwd":dir.path(),"tool_input":{"command":"cat small.txt"}});
    assert_eq!(hook(dir.path(), "codex", event.clone()), json!({}));
    fs::write(&file, vec![b'x'; 2049]).unwrap();
    assert!(
        hook(dir.path(), "codex", event.clone())["hookSpecificOutput"]["updatedInput"]["command"]
            .as_str()
            .unwrap()
            .contains("run --auto")
    );
    event["tool_input"]["command"] = json!("cat missing.txt");
    assert_ne!(hook(dir.path(), "codex", event.clone()), json!({}));
    event["tool_input"]["command"] = json!("cat .");
    assert_ne!(hook(dir.path(), "codex", event), json!({}));
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

fn opencode_plugin(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("opencode/plugin/scopelet.js")
}
#[test]
fn opencode_install_is_idempotent_reversible_and_never_replaces_a_foreign_plugin() {
    let dir = tempfile::tempdir().unwrap();
    let plugin = opencode_plugin(dir.path());
    cli(dir.path())
        .args(["install", "--agent", "opencode"])
        .assert()
        .success();
    let installed = fs::read_to_string(&plugin).unwrap();
    assert!(installed.contains("ScopeletPlugin"));
    assert!(
        installed
            .contains(&serde_json::to_string(&dir.path().join("config/bin/scopelet")).unwrap())
    );
    assert!(!installed.contains("__SCOPELET_"));
    cli(dir.path())
        .args(["install", "--agent", "opencode"])
        .assert()
        .success();
    assert_eq!(fs::read_to_string(&plugin).unwrap(), installed);
    assert!(!dir.path().join("config/backups").exists());
    let doctor = cli(dir.path()).arg("doctor").output().unwrap();
    let health: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    let hosts = health["integration"]["hosts"].as_array().unwrap();
    assert_eq!(hosts.len(), 3);
    assert_eq!(hosts[2]["host"], "opencode");
    assert_eq!(hosts[2]["hooks_configured"], true);
    cli(dir.path())
        .args(["uninstall", "--agent", "opencode"])
        .assert()
        .success();
    assert!(!plugin.exists());
    assert!(dir.path().join("opencode/plugin").is_dir());
    let foreign = "export const OtherPlugin = async () => ({});\n";
    fs::write(&plugin, foreign).unwrap();
    cli(dir.path())
        .args(["install", "--agent", "all"])
        .assert()
        .failure();
    assert_eq!(fs::read_to_string(&plugin).unwrap(), foreign);
    cli(dir.path())
        .args(["uninstall", "--agent", "all"])
        .assert()
        .failure();
    assert!(plugin.exists());
}
#[test]
fn opencode_hook_compresses_large_bash_output_and_carries_the_mode() {
    let dir = tempfile::tempdir().unwrap();
    let large = json!({"hook_event_name":"ToolOutput","tool_name":"Bash","tool_input":{"command":"npm test"},"tool_response":{"output":"noise\n".repeat(2000)}});
    let out = hook(dir.path(), "opencode", large.clone());
    let text = out["output"].as_str().unwrap();
    assert!(text.contains("[scopelet compact-v1"));
    assert!(text.len() < 6000);
    let mut small = large.clone();
    small["tool_response"]["output"] = json!("x".repeat(2048));
    assert_eq!(hook(dir.path(), "opencode", small), json!({}));
    let mut wrapped = large.clone();
    wrapped["tool_input"]["command"] = json!("scopelet run -- npm test");
    assert_eq!(hook(dir.path(), "opencode", wrapped), json!({}));
    let mut other_tool = large.clone();
    other_tool["tool_name"] = json!("Read");
    assert_eq!(hook(dir.path(), "opencode", other_tool), json!({}));
    let system = json!({"hook_event_name":"SystemPrompt","session_id":"s1"});
    assert!(
        hook(dir.path(), "opencode", system.clone())["system"]
            .as_str()
            .unwrap()
            .contains("Scopelet auto")
    );
    // Unlike hook context on the other hosts, the system prompt is rebuilt every step.
    assert_ne!(hook(dir.path(), "opencode", system.clone()), json!({}));
    cli(dir.path()).args(["mode", "caveman"]).assert().success();
    assert!(
        hook(dir.path(), "opencode", system.clone())["system"]
            .as_str()
            .unwrap()
            .contains("telegraphic")
    );
    cli(dir.path()).args(["mode", "off"]).assert().success();
    assert_eq!(hook(dir.path(), "opencode", system), json!({}));
    assert_eq!(hook(dir.path(), "opencode", large), json!({}));
}
#[test]
fn opencode_plugin_file_drives_the_binary_from_node() {
    let Some(node) = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|p| p.join("node"))
            .find(|p| p.is_file())
    }) else {
        eprintln!("node not found; skipping the plugin smoke test");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    cli(dir.path())
        .args(["install", "--agent", "opencode"])
        .assert()
        .success();
    let plugin = opencode_plugin(dir.path());
    let script = format!(
        r#"import {{ ScopeletPlugin }} from {};
const hooks = await ScopeletPlugin({{ directory: process.cwd() }});
const output = {{ title: "npm test", output: "noise\n".repeat(2000), metadata: {{}} }};
await hooks["tool.execute.after"]({{ tool: "bash", sessionID: "s1", callID: "c1", args: {{ command: "npm test" }} }}, output);
const small = {{ title: "ls", output: "fine\n", metadata: {{}} }};
await hooks["tool.execute.after"]({{ tool: "bash", sessionID: "s1", callID: "c2", args: {{ command: "ls" }} }}, small);
const system = {{ system: ["base"] }};
await hooks["experimental.chat.system.transform"]({{ sessionID: "s1" }}, system);
console.log(JSON.stringify({{ compressed: output.output, small: small.output, title: output.title, system }}));
"#,
        serde_json::to_string(&plugin).unwrap()
    );
    let entry = dir.path().join("smoke.mjs");
    fs::write(&entry, script).unwrap();
    let out = std::process::Command::new(node)
        .arg(&entry)
        .env("SCOPELET_CACHE_DIR", dir.path().join("cache"))
        .env("SCOPELET_COMPACT_VERSION", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        result["compressed"]
            .as_str()
            .unwrap()
            .contains("[scopelet compact-v1")
    );
    assert_eq!(result["small"], "fine\n");
    assert_eq!(result["title"], "npm test");
    assert_eq!(result["system"]["system"][0], "base");
    assert!(
        result["system"]["system"][1]
            .as_str()
            .unwrap()
            .contains("Scopelet auto")
    );
}
