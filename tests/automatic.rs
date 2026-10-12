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
#[test]
fn uninstall_reports_its_own_note_for_every_host() {
    let dir = tempfile::tempdir().unwrap();
    let report = |args: &[&str]| -> Value {
        let out = cli(dir.path()).args(args).output().unwrap();
        assert!(out.status.success());
        serde_json::from_slice(&out.stdout).unwrap()
    };
    for agent in ["claude", "codex", "opencode", "all"] {
        let installed = report(&["install", "--agent", agent]);
        assert_eq!(installed["installed"], true);
        assert!(
            installed["note"]
                .as_str()
                .unwrap()
                .contains("after installation")
        );
        let removed = report(&["uninstall", "--agent", agent]);
        assert_eq!(removed["installed"], false);
        let note = removed["note"].as_str().unwrap();
        assert!(
            !note.contains("installation") && !note.contains("/hooks"),
            "{agent}: {note}"
        );
        assert!(
            note.contains("stop running the removed hooks"),
            "{agent}: {note}"
        );
    }
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
        assert!(!store_opened(dir.path()));
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
/// Whether the cache store was opened: only the event journal may be written.
fn store_opened(dir: &std::path::Path) -> bool {
    ["cache/blobs", "cache/artifacts"]
        .iter()
        .any(|d| dir.join(d).exists())
}

#[test]
fn small_automatic_outputs_do_not_open_cache() {
    let dir = tempfile::tempdir().unwrap();
    let event = json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_response":{"stdout":"x".repeat(2048),"stderr":"warning\r\n","interrupted":false,"isImage":false}});
    assert_eq!(hook(dir.path(), "claude", event), json!({}));
    assert!(!store_opened(dir.path()));
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
    assert!(!store_opened(dir.path()));
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
    // OpenCode 2 truncates `seq 1 3000` before the hook: the event carries the
    // last 2000 lines and a notice naming the whole output it saved under
    // `$XDG_DATA_HOME/opencode/shell/<project id>`.
    let saved = dir.path().join("data/opencode/shell/p1");
    fs::create_dir_all(&saved).unwrap();
    let full: String = (1..=3000).map(|i| format!("{i}\n")).collect();
    let tail: String = (1001..=3000).map(|i| format!("{i}\n")).collect();
    let truncated = |file: &std::path::Path| {
        format!(
            "{tail}\n[showing lines 1001-3000 of 3000; full output saved to {}]",
            file.display()
        )
    };
    fs::write(saved.join("sh_01truncated.out"), &full).unwrap();
    // A path named by the visible text is followed only into OpenCode's own directory.
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("sh_02outside.out"), format!("forged\n{full}")).unwrap();
    std::os::unix::fs::symlink(
        outside.join("sh_02outside.out"),
        saved.join("sh_03link.out"),
    )
    .unwrap();
    let texts = json!({
        "truncated": truncated(&saved.join("sh_01truncated.out")),
        "outside": truncated(&outside.join("sh_02outside.out")),
        "link": truncated(&saved.join("sh_03link.out")),
    });
    let script = format!(
        r#"import plugin from {};
// OpenCode 1: the legacy `server` entrypoint returns the hook map.
const hooks = await plugin.server({{ directory: process.cwd() }});
const output = {{ title: "npm test", output: "noise\n".repeat(2000), metadata: {{}} }};
await hooks["tool.execute.after"]({{ tool: "bash", sessionID: "s1", callID: "c1", args: {{ command: "npm test" }} }}, output);
const small = {{ title: "ls", output: "fine\n", metadata: {{}} }};
await hooks["tool.execute.after"]({{ tool: "bash", sessionID: "s1", callID: "c2", args: {{ command: "ls" }} }}, small);
const system = {{ system: ["base"] }};
await hooks["experimental.chat.system.transform"]({{ sessionID: "s1" }}, system);
// OpenCode 2: `setup` registers the same work on the owning domains; capture
// the callbacks through a stub context and drive them like OpenCode would.
const registered = {{}};
const ctx = {{
  location: {{ directory: process.cwd(), project: {{ id: "p1", directory: process.cwd() }} }},
  session: {{ hook: async (name, cb) => {{ registered["session." + name] = cb; }} }},
  tool: {{ hook: async (name, cb) => {{ registered["tool." + name] = cb; }} }},
}};
await plugin.setup(ctx);
const v2Event = (tool, command, text) => ({{ tool, sessionID: "s1", agent: "build", messageID: "m1", id: "c1", input: {{ command }}, status: "completed", result: {{ output: {{ exit: 0, truncated: false, output: text, status: "completed" }}, content: [{{ type: "text", text }}], metadata: {{ status: "completed", truncated: false, exit: 0 }} }} }});
const v2Output = v2Event("shell", "npm test", "noise\n".repeat(2000));
await registered["tool.execute.after"](v2Output);
const v2Small = v2Event("shell", "ls", "fine\n");
await registered["tool.execute.after"](v2Small);
const v2Bash = v2Event("bash", "npm test", "noise\n".repeat(2000));
await registered["tool.execute.after"](v2Bash);
const v2System = {{ system: [{{ type: "text", text: "base" }}] }};
await registered["session.context"]({{ sessionID: "s1", system: v2System.system }});
// The truncated shape observed from OpenCode 2.0.26 for `seq 1 3000`.
const texts = {};
const truncated = {{}};
for (const [name, text] of Object.entries(texts)) {{
  const event = v2Event("shell", "seq 1 3000", text);
  event.result.output.truncated = true;
  event.result.metadata.truncated = true;
  await registered["tool.execute.after"](event);
  truncated[name] = event.result;
}}
console.log(JSON.stringify({{ compressed: output.output, small: small.output, title: output.title, system, v2: {{ compressed: v2Output.result, small: v2Small.result, bash: v2Bash.result, system: v2System, truncated }} }}));
"#,
        serde_json::to_string(&plugin).unwrap(),
        texts
    );
    let entry = dir.path().join("smoke.mjs");
    fs::write(&entry, script).unwrap();
    let out = std::process::Command::new(node)
        .arg(&entry)
        .env("SCOPELET_CACHE_DIR", dir.path().join("cache"))
        .env("SCOPELET_COMPACT_VERSION", "1")
        .env("XDG_DATA_HOME", dir.path().join("data"))
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
    // The OpenCode 2 entrypoint compresses the text parts the model reads.
    let v2 = &result["v2"]["compressed"];
    assert!(
        v2["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("[scopelet compact-v1")
    );
    assert_eq!(v2["output"]["output"], v2["content"][0]["text"]);
    assert_eq!(result["v2"]["small"]["content"][0]["text"], "fine\n");
    assert_eq!(result["v2"]["small"]["output"]["output"], "fine\n");
    assert!(
        result["v2"]["bash"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("[scopelet compact-v1")
    );
    assert_eq!(result["v2"]["system"]["system"][0]["text"], "base");
    assert!(
        result["v2"]["system"]["system"][1]["text"]
            .as_str()
            .unwrap()
            .contains("Scopelet auto")
    );
    // A truncated result is compressed from the saved whole output, so its
    // artifact recovers the lines OpenCode cut; untrusted paths keep the tail.
    let find = |name: &str, pattern: &str| -> Value {
        let result = &result["v2"]["truncated"][name];
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("[scopelet compact-v1"), "{name}: {text}");
        assert_eq!(result["output"]["output"], result["content"][0]["text"]);
        let artifact = text
            .split_whitespace()
            .find(|s| s.starts_with("artifact:"))
            .unwrap();
        let out = cli(dir.path())
            .args(["expand", artifact, "--find", pattern, "--regex"])
            .output()
            .unwrap();
        assert!(out.status.success());
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["records"].clone()
    };
    let recovered = find("truncated", "^1$");
    assert_eq!(recovered[0]["start_line"], 1, "{recovered}");
    assert!(recovered[0]["text"].as_str().unwrap().starts_with("1\n2\n"));
    for name in ["outside", "link"] {
        assert_eq!(find(name, "^1$"), json!([]), "{name}");
        assert_eq!(find(name, "^forged$"), json!([]), "{name}");
        assert_eq!(find(name, "^1001$")[0]["start_line"], 1, "{name}");
    }
}

fn rewritten(dir: &std::path::Path, command: &str) -> Option<String> {
    let event = json!({"hook_event_name":"PreToolUse","tool_name":"Bash","cwd":dir,"tool_input":{"command":command}});
    let result = hook(dir, "codex", event);
    result["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .map(str::to_owned)
}

/// Commands are judged on their parsed words, not on raw characters, so
/// quoting no longer hides a recognized command, and a pipeline ending in a
/// filter that already bounds the output stays native.
#[test]
fn codex_grammar_accepts_quoted_and_composed_commands_it_understands() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    for (command, executed) in [
        ("cargo test", "'cargo' 'test'"),
        ("cat 'a && b'", "'cat' 'a && b'"),
        (r"rg 'fn \w+\(' src", r"'rg' 'fn \w+\(' 'src'"),
        (
            "cargo check && cargo test",
            r"'/bin/sh' '-c' ''\''cargo'\'' '\''check'\'' && '\''cargo'\'' '\''test'\'''",
        ),
        ("cargo test 2>&1", "'/bin/sh' '-c' 'cargo test 2>&1'"),
        (
            "RUST_BACKTRACE=1 cargo test",
            "'/bin/sh' '-c' 'RUST_BACKTRACE=1 cargo test'",
        ),
        (
            "cd sub && cargo test",
            "'/bin/sh' '-c' 'cd sub && cargo test'",
        ),
        ("npm test || true", "'/bin/sh' '-c' 'npm test || true'"),
        (
            "cargo test 2>&1 | grep -v Compiling",
            "'/bin/sh' '-c' 'cargo test 2>&1 | grep -v Compiling'",
        ),
        (
            "go test ./... 2>/dev/null",
            "'/bin/sh' '-c' 'go test ./... 2>/dev/null'",
        ),
    ] {
        let command_line =
            rewritten(dir.path(), command).unwrap_or_else(|| panic!("left native: {command}"));
        assert!(command_line.contains("run --auto"), "{command_line}");
        assert!(
            command_line.ends_with(&format!(" -- {executed}")),
            "{command}: {command_line}"
        );
    }
    for command in [
        "cargo test 2>&1 | tail -n 20",
        "cargo test | head",
        "cargo test | wc -l",
        "cargo test | grep -c ok",
        "rg foo | rg -l bar",
        "cargo test | tee log",
        "cargo test | xargs rm",
        "cargo test | awk '{print}'",
        "cargo test | sed -i s/a/b/ x",
        "cargo test | sort -o out",
        "LD_PRELOAD=/tmp/x.so cargo test",
        "GIT_EXTERNAL_DIFF=/tmp/x git diff",
        "PAGER=/tmp/x git log",
        "cd sub",
        "cd sub || true",
        "cd a b && cargo test",
        "cd sub && cd other",
        "echo x | cat",
        "cargo test > out.txt",
        "cargo test; rm x",
        "cargo test `id`",
        "cargo test && rm -rf target",
    ] {
        assert_eq!(rewritten(dir.path(), command), None, "rewritten: {command}");
    }
}

/// Whatever the hook hands to `/bin/sh -c`, `run --auto` reproduces: same
/// stdout, stderr and exit status for outputs small enough to stay exact.
#[test]
fn run_auto_through_sh_matches_the_shell_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    for script in [
        r"printf 'out\n'; printf 'err\n' >&2; exit 3",
        r"printf 'a\n' 2>&1 && printf 'b\n' >&2",
        "false || true",
        r"false && printf 'never\n'",
        r"printf 'x\ny\n' | grep y",
        r#"FOO=1 sh -c 'printf "%s\n" "$FOO"'"#,
        r"printf 'gone\n' >/dev/null; printf 'kept\n' 2>/dev/null",
        "cd / && pwd",
        "exit 42",
    ] {
        let native = std::process::Command::new("/bin/sh")
            .args(["-c", script])
            .output()
            .unwrap();
        let wrapped = cli(dir.path())
            .args(["run", "--auto", "--", "/bin/sh", "-c", script])
            .output()
            .unwrap();
        assert_eq!(wrapped.status.code(), native.status.code(), "{script}");
        assert_eq!(wrapped.stdout, native.stdout, "{script}");
        assert_eq!(wrapped.stderr, native.stderr, "{script}");
    }
}

#[test]
fn guidance_names_the_pinned_binary_for_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let event = json!({"hook_event_name":"SessionStart","session_id":"session-path"});
    let context = hook(dir.path(), "claude", event)["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_owned();
    let pinned = dir.path().join("config/bin/scopelet");
    assert!(
        context.contains(&format!("'{}' expand ID --find TEXT", pinned.display())),
        "{context}"
    );
    assert!(context.contains("last"), "{context}");
}

#[test]
fn doctor_reports_a_stale_pinned_binary() {
    let dir = tempfile::tempdir().unwrap();
    cli(dir.path())
        .args(["install", "--agent", "opencode"])
        .assert()
        .success();
    let plugin = fs::read_to_string(opencode_plugin(dir.path())).unwrap();
    assert!(!plugin.contains("__SCOPELET_"), "{plugin}");
    assert!(plugin.contains("const SMALL = 2048;"), "{plugin}");
    let doctor = |dir: &std::path::Path| -> Value {
        let out = cli(dir).arg("doctor").output().unwrap();
        assert!(out.status.success());
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["integration"].clone()
    };
    let status = doctor(dir.path());
    assert_eq!(status["binary_current"], true, "{status}");
    assert_eq!(
        status["binary_version"], status["running_version"],
        "{status}"
    );
    assert!(status.get("binary_note").is_none());

    // An older copy left behind by a previous installation.
    let pinned = dir.path().join("config/bin/scopelet");
    fs::write(&pinned, "#!/bin/sh\necho scopelet 0.5.0\n").unwrap();
    let status = doctor(dir.path());
    assert_eq!(status["binary_current"], false, "{status}");
    assert_eq!(status["binary_version"], "scopelet 0.5.0");
    assert!(
        status["binary_note"]
            .as_str()
            .unwrap()
            .contains("install --agent all")
    );
}

#[test]
fn the_event_journal_records_decisions_and_can_be_turned_off() {
    let dir = tempfile::tempdir().unwrap();
    let event = json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":"npm test"},"tool_response":{"stdout":"x".repeat(100),"stderr":"","interrupted":false,"isImage":false}});
    let journal = |dir: &std::path::Path| -> Vec<Value> {
        let folder = dir.join("cache/events");
        std::fs::read_dir(&folder)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
                    .flat_map(|e| {
                        fs::read_to_string(e.path())
                            .unwrap()
                            .lines()
                            .map(|l| serde_json::from_str::<Value>(l).unwrap())
                            .collect::<Vec<_>>()
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    assert_eq!(hook(dir.path(), "claude", event.clone()), json!({}));
    let events = journal(dir.path());
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["reason"], "small");
    assert_eq!(events[0]["host"], "claude");
    assert_eq!(events[0]["bytes_in"], 100);
    // Nothing that could identify the command or its output.
    let line = events[0].to_string();
    assert!(!line.contains("npm") && !line.contains("xxxx"), "{line}");
    assert!(line.len() < 512);

    let quiet = tempfile::tempdir().unwrap();
    let output = assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("scopelet"))
        .env("SCOPELET_EVENTS", "0")
        .env("SCOPELET_CONFIG_DIR", quiet.path().join("config"))
        .env("SCOPELET_CACHE_DIR", quiet.path().join("cache"))
        .args(["hook", "claude"])
        .write_stdin(event.to_string())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(!quiet.path().join("cache").exists());
}

#[test]
fn stats_count_compressions_pass_throughs_and_later_expansions() {
    let dir = tempfile::tempdir().unwrap();
    let big: String = (0..2000).map(|i| format!("line {i} of noise\n")).collect();
    let run = |stdin: &str| -> Vec<u8> {
        cli(dir.path())
            .env("SCOPELET_COMPACT_VERSION", "3")
            .arg("compress")
            .write_stdin(stdin.to_owned())
            .output()
            .unwrap()
            .stdout
    };
    let view = String::from_utf8(run(&big)).unwrap();
    run("tiny\n");
    let artifact = view
        .split_whitespace()
        .find(|s| s.starts_with("artifact:"))
        .unwrap();
    cli(dir.path())
        .args(["expand", &artifact[9..21], "--manifest"])
        .assert()
        .success();
    let out = cli(dir.path())
        .args(["stats", "--days", "1"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stats: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(stats["compressions"], 1, "{stats}");
    assert_eq!(stats["pass_through"]["small"], 1, "{stats}");
    assert_eq!(stats["expands"], 1, "{stats}");
    assert_eq!(stats["expanded_after_compression"], 1, "{stats}");
    assert_eq!(stats["expand_rate"], 1.0, "{stats}");
    assert_eq!(
        stats["bytes_saved"].as_u64().unwrap(),
        (big.len() - view.len()) as u64
    );
}

fn session_state(dir: &std::path::Path, session: &str, agent: Option<&str>) -> Option<Value> {
    let key = scopelet::session::key(session, agent);
    fs::read(dir.join("config/sessions").join(key))
        .ok()
        .map(|bytes| serde_json::from_slice(&bytes).unwrap())
}

/// Each context keeps a private JSON state: the main conversation under its
/// session, a subagent under its session and agent. A marker written by an
/// earlier version (the bare mode) still counts as announced.
#[test]
fn session_state_is_kept_per_context_and_reads_the_old_marker() {
    let dir = tempfile::tempdir().unwrap();
    let start = json!({"hook_event_name":"SessionStart","session_id":"s1","source":"startup"});
    assert_ne!(hook(dir.path(), "claude", start), json!({}));
    let state = session_state(dir.path(), "s1", None).unwrap();
    assert_eq!(state["mode"], "default");
    assert_eq!(state["seen"], json!([]));
    assert!(session_state(dir.path(), "s1", Some("agent-7")).is_none());
    let prompt =
        json!({"hook_event_name":"UserPromptSubmit","session_id":"s1","agent_id":"agent-7"});
    assert_ne!(hook(dir.path(), "claude", prompt.clone()), json!({}));
    assert_eq!(
        session_state(dir.path(), "s1", Some("agent-7")).unwrap()["mode"],
        "default"
    );
    assert_eq!(hook(dir.path(), "claude", prompt), json!({}));

    let old = dir
        .path()
        .join("config/sessions")
        .join(scopelet::session::key("old", None));
    fs::write(&old, b"\"default\"").unwrap();
    let prompt = json!({"hook_event_name":"UserPromptSubmit","session_id":"old"});
    assert_eq!(hook(dir.path(), "claude", prompt), json!({}));
}

/// A compaction or a clear empties what the context was shown; a resume
/// keeps it. A clear also drops the anchors of earlier prompts.
#[test]
fn session_start_after_compact_or_clear_forgets_shown_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("config/sessions");
    fs::create_dir_all(&folder).unwrap();
    for (source, kept) in [("resume", true), ("compact", false), ("clear", false)] {
        let key = scopelet::session::key("s2", None);
        let seeded =
            json!({"mode":"default","seen":["0123456789abcdef"],"anchors":["parse_header"]});
        fs::write(folder.join(&key), seeded.to_string()).unwrap();
        let start = json!({"hook_event_name":"SessionStart","session_id":"s2","source":source});
        assert_ne!(hook(dir.path(), "claude", start), json!({}));
        let state = session_state(dir.path(), "s2", None).unwrap();
        assert_eq!(
            state["seen"] == json!(["0123456789abcdef"]),
            kept,
            "{source}"
        );
        assert_eq!(
            state["anchors"] == json!(["parse_header"]),
            source != "clear",
            "{source}"
        );
    }
}

/// Codex hands its context key to the wrapped run, which accepts only hex.
#[test]
fn codex_passes_the_context_key_to_the_wrapped_run() {
    let dir = tempfile::tempdir().unwrap();
    let mut event = json!({"hook_event_name":"PreToolUse","tool_name":"Bash","session_id":"codex-1","tool_input":{"command":"cargo test"}});
    let key = scopelet::session::key("codex-1", None);
    let rewritten = |event: Value| -> String {
        hook(dir.path(), "codex", event)["hookSpecificOutput"]["updatedInput"]["command"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let command = rewritten(event.clone());
    assert!(
        command.contains(&format!(" --session {} ", &key[..16])),
        "{command}"
    );
    event.as_object_mut().unwrap().remove("session_id");
    let command = rewritten(event);
    assert!(!command.contains("--session"), "{command}");
    cli(dir.path())
        .args(["run", "--auto", "--session", &key[..16], "--", "true"])
        .assert()
        .success();
    cli(dir.path())
        .args(["run", "--auto", "--session", "not-hex", "--", "true"])
        .assert()
        .failure();
}

fn hook_v4(dir: &std::path::Path, agent: &str, event: Value) -> Value {
    let result = cli(dir)
        .env("SCOPELET_COMPACT_VERSION", "4")
        .args(["hook", agent])
        .write_stdin(event.to_string())
        .output()
        .unwrap();
    assert!(result.status.success());
    serde_json::from_slice(&result.stdout).unwrap()
}

/// The text Claude would show for a PostToolUse result: the replaced stream,
/// or the original when the hook left it alone.
fn shown(event: &Value, result: &Value, stream: &str) -> String {
    let replaced = &result["hookSpecificOutput"]["updatedToolOutput"][stream];
    replaced
        .as_str()
        .unwrap_or_else(|| event["tool_response"][stream].as_str().unwrap())
        .to_owned()
}

fn bash_result(session: &str, command: &str, stdout: &str) -> Value {
    json!({"hook_event_name":"PostToolUse","session_id":session,"tool_name":"Bash","tool_input":{"command":command},"tool_response":{"stdout":stdout,"stderr":"","interrupted":false,"isImage":false}})
}

/// The original bytes behind a repeat reference, through `expand`.
fn recovered(dir: &std::path::Path, text: &str) -> Vec<u8> {
    let id = text
        .strip_prefix("[scopelet v4 ")
        .and_then(|rest| rest.get(..16))
        .unwrap();
    let manifest = cli(dir)
        .args(["expand", id, "--manifest"])
        .output()
        .unwrap();
    assert!(manifest.status.success(), "{manifest:?}");
    let manifest: Value = serde_json::from_slice(&manifest.stdout).unwrap();
    let blob = manifest["snapshots"][0]["blob"].as_str().unwrap();
    cli(dir)
        .args(["expand", blob, "--raw"])
        .output()
        .unwrap()
        .stdout
}

/// An output identical to one this context was already shown becomes a
/// one-line reference that recovers the original, whether the first
/// occurrence was a view or passed through unchanged.
#[test]
fn repeat_outputs_become_a_reference_to_what_was_shown() {
    let dir = tempfile::tempdir().unwrap();
    let log: String = (0..400)
        .map(|i| format!("worker {i} finished batch\n"))
        .collect();
    let event = bash_result("r1", "npm test", &log);
    let first = shown(
        &event,
        &hook_v4(dir.path(), "claude", event.clone()),
        "stdout",
    );
    assert!(first.starts_with("[scopelet v4 "), "{first}");
    assert!(!first.contains("repeat:"), "{first}");
    let second = shown(
        &event,
        &hook_v4(dir.path(), "claude", event.clone()),
        "stdout",
    );
    let id = &first[13..29];
    assert_eq!(
        second,
        format!(
            "[scopelet v4 {id} repeat: identical to output already shown in this conversation (400 lines); expand ID]\n"
        )
    );
    assert_eq!(recovered(dir.path(), &second), log.as_bytes());

    // A file read up to 16 KiB is shown whole the first time, then referenced.
    let source = "fn parse(input: &str) -> usize { input.len() }\n".repeat(80);
    let read = bash_result("r1", "cat src/parse.rs", &source);
    assert_eq!(hook_v4(dir.path(), "claude", read.clone()), json!({}));
    let again = shown(
        &read,
        &hook_v4(dir.path(), "claude", read.clone()),
        "stdout",
    );
    assert!(again.contains(" repeat: identical"), "{again}");
    assert_eq!(recovered(dir.path(), &again), source.as_bytes());

    // Small outputs are never referenced: they cost less than the marker.
    let small = bash_result("r1", "git status", "On branch main\n");
    assert_eq!(hook_v4(dir.path(), "claude", small.clone()), json!({}));
    assert_eq!(hook_v4(dir.path(), "claude", small), json!({}));
}

/// Repeats are judged per context: another subagent, a compaction, a
/// persisted preview, a missing session or an earlier compact version never
/// produce a reference.
#[test]
fn repeat_references_stay_inside_one_uncompacted_context() {
    let dir = tempfile::tempdir().unwrap();
    let log: String = (0..400)
        .map(|i| format!("worker {i} finished batch\n"))
        .collect();
    let is_repeat = |event: &Value| {
        shown(
            event,
            &hook_v4(dir.path(), "claude", event.clone()),
            "stdout",
        )
        .contains("repeat:")
    };
    let main = bash_result("r2", "npm test", &log);
    assert!(!is_repeat(&main));
    let mut subagent = main.clone();
    subagent["agent_id"] = json!("explorer-1");
    assert!(!is_repeat(&subagent));
    assert!(is_repeat(&subagent));
    assert!(is_repeat(&main));
    let compact = json!({"hook_event_name":"SessionStart","session_id":"r2","source":"compact"});
    hook_v4(dir.path(), "claude", compact);
    assert!(!is_repeat(&main));
    assert!(is_repeat(&main));

    // Only a preview of a persisted output reached the model.
    let mut persisted = bash_result("r3", "npm test", &log);
    persisted["tool_response"]["persistedOutputPath"] = json!("/synthetic/tool-results/a.txt");
    assert_eq!(hook_v4(dir.path(), "claude", persisted), json!({}));
    assert!(!is_repeat(&bash_result("r3", "npm test", &log)));

    let mut anonymous = bash_result("r4", "npm test", &log);
    anonymous.as_object_mut().unwrap().remove("session_id");
    assert!(!is_repeat(&anonymous));
    assert!(!is_repeat(&anonymous));

    let v3 = bash_result("r5", "npm test", &log);
    for _ in 0..2 {
        let result = cli(dir.path())
            .env("SCOPELET_COMPACT_VERSION", "3")
            .args(["hook", "claude"])
            .write_stdin(v3.to_string())
            .output()
            .unwrap();
        let result: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert!(shown(&v3, &result, "stdout").starts_with("[scopelet compact-v3 "));
    }
}

/// A repeat is journaled with the bytes it saved, and nothing of its content.
#[test]
fn repeat_references_are_journaled() {
    let dir = tempfile::tempdir().unwrap();
    let log: String = (0..400)
        .map(|i| format!("worker {i} finished batch\n"))
        .collect();
    let event = bash_result("r6", "npm test", &log);
    hook_v4(dir.path(), "claude", event.clone());
    let marker = shown(
        &event,
        &hook_v4(dir.path(), "claude", event.clone()),
        "stdout",
    );
    let folder = dir.path().join("cache/events");
    let events: Vec<Value> = fs::read_dir(folder)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .flat_map(|e| {
            fs::read_to_string(e.path())
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str::<Value>(l).unwrap())
                .collect::<Vec<_>>()
        })
        .collect();
    let repeat = events.iter().find(|e| e["reason"] == "repeat").unwrap();
    assert_eq!(repeat["bytes_in"], log.len());
    assert_eq!(repeat["bytes_out"], marker.len());
    assert_eq!(repeat["version"], 4);
    assert!(!repeat.to_string().contains("worker"), "{repeat}");
}

/// A Grep PostToolUse event captured from Claude Code 2.1.296.
fn grep_fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// Claude Code's Grep in content mode: its `content` text is compressed as
/// search results and every other field of the result is kept as it was.
#[test]
fn grep_content_results_are_compressed_and_other_shapes_pass() {
    let dir = tempfile::tempdir().unwrap();
    let mut event = grep_fixture("claude_grep_content");
    let words: Vec<String> = (0..600)
        .map(|i| {
            i.to_string()
                .bytes()
                .map(|b| (b'a' + (b - b'0')) as char)
                .collect()
        })
        .collect();
    let content: String = (0..600)
        .map(|i| {
            format!(
                "src/mod{}.rs:{}:    let {} = refresh_token();\n",
                i / 20,
                2 + i % 20,
                words[i]
            )
        })
        .collect();
    event["tool_response"]["content"] = json!(content.trim_end());
    event["tool_response"]["numLines"] = json!(600);
    let result = hook_v4(dir.path(), "claude", event.clone());
    let output = &result["hookSpecificOutput"]["updatedToolOutput"];
    let view = output["content"].as_str().unwrap();
    assert!(view.starts_with("[scopelet v4 "), "{view}");
    assert!(view.contains(" file=src/mod0.rs"), "{view}");
    assert!(view.len() < content.len() / 2, "{}", view.len());
    for field in ["mode", "numFiles", "filenames", "numLines", "totalLines"] {
        assert_eq!(output[field], event["tool_response"][field], "{field}");
    }
    assert_eq!(recovered(dir.path(), view), content.trim_end().as_bytes());

    // Shown twice in one context, it becomes a repeat reference.
    let again = hook_v4(dir.path(), "claude", event.clone());
    let again = again["hookSpecificOutput"]["updatedToolOutput"]["content"]
        .as_str()
        .unwrap();
    assert!(again.contains(" repeat: identical"), "{again}");

    // Other modes and unknown shapes pass through, however large.
    let mut files = grep_fixture("claude_grep_files");
    files["tool_response"]["filenames"] = json!(vec!["src/x.rs"; 3000]);
    assert_eq!(hook_v4(dir.path(), "claude", files), json!({}));
    let mut count = grep_fixture("claude_grep_count");
    count["tool_response"]["content"] = json!("src/mod1.rs:1\n".repeat(400));
    assert_eq!(hook_v4(dir.path(), "claude", count), json!({}));
    let mut unknown = event.clone();
    unknown["tool_response"] = json!({"text": content});
    assert_eq!(hook_v4(dir.path(), "claude", unknown), json!({}));
    // Codex has no Grep tool to compress.
    assert_eq!(hook_v4(dir.path(), "codex", event), json!({}));
}

/// The installed Claude PostToolUse hook matches Bash and Grep; reinstalling
/// changes nothing and uninstalling removes it.
#[test]
fn grep_matcher_is_installed_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    cli(dir.path())
        .args(["install", "--agent", "claude"])
        .assert()
        .success();
    let path = dir.path().join("claude/settings.json");
    let installed = fs::read(&path).unwrap();
    let value: Value = serde_json::from_slice(&installed).unwrap();
    assert_eq!(value["hooks"]["PostToolUse"][0]["matcher"], "Bash|Grep");
    cli(dir.path())
        .args(["install", "--agent", "claude"])
        .assert()
        .success();
    assert_eq!(fs::read(&path).unwrap(), installed);
    cli(dir.path())
        .args(["uninstall", "--agent", "claude"])
        .assert()
        .success();
    let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert!(value.get("hooks").is_none(), "{value}");
}

/// The guidance text with its pinned binary path counted as the usual
/// `'~/.config/scopelet/bin/scopelet'` (45 bytes quoted), whatever the test's
/// temporary directory.
fn guidance_bytes(dir: &std::path::Path, text: &str) -> usize {
    let pinned = format!("'{}'", dir.join("config/bin/scopelet").display());
    assert!(text.contains(&pinned), "{text}");
    text.len() - pinned.len() + 45
}

/// Default and caveman guidance stay short, carry the reuse sentence and
/// the recovery syntax; off says only that it is off.
#[test]
fn guidance_is_short_and_asks_for_the_smallest_change() {
    let dir = tempfile::tempdir().unwrap();
    let start = |session: &str| -> String {
        hook(
            dir.path(),
            "claude",
            json!({"hook_event_name":"SessionStart","session_id":session}),
        )["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let reuse = "Before writing code: reuse existing code, stdlib or installed deps; make the smallest change that passes existing checks.";
    let default = start("g1");
    assert!(guidance_bytes(dir.path(), &default) <= 380, "{default}");
    assert!(default.contains(reuse), "{default}");
    assert!(
        default.contains(" expand ID --find TEXT [--regex] [--ignore-case]"),
        "{default}"
    );
    cli(dir.path()).args(["mode", "caveman"]).assert().success();
    let caveman = start("g2");
    assert!(guidance_bytes(dir.path(), &caveman) <= 430, "{caveman}");
    assert!(caveman.contains(reuse), "{caveman}");
    assert!(caveman.contains("telegraphic"), "{caveman}");
    cli(dir.path()).args(["mode", "off"]).assert().success();
    let off = start("g3");
    assert!(!off.contains(reuse), "{off}");
}

/// A subagent starts with the recovery line and the report its parent will
/// read; mode off injects nothing.
#[test]
fn guidance_reaches_subagents_at_their_start() {
    let dir = tempfile::tempdir().unwrap();
    let event = json!({"hook_event_name":"SubagentStart","session_id":"s","agent_id":"a1","agent_type":"Explore"});
    let out = hook(dir.path(), "claude", event.clone());
    assert_eq!(out["hookSpecificOutput"]["hookEventName"], "SubagentStart");
    let text = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(text.contains(" expand ID --find TEXT"), "{text}");
    assert!(
        text.contains("Final report: conclusions with file:line evidence; do not restate commands or raw output."),
        "{text}"
    );
    cli(dir.path()).args(["mode", "off"]).assert().success();
    assert_eq!(hook(dir.path(), "claude", event), json!({}));
}

/// Install adds SubagentStart for Claude Code only, reinstalling changes
/// nothing, doctor requires it and uninstall removes it.
#[test]
fn guidance_hooks_install_and_uninstall_subagent_start() {
    let dir = tempfile::tempdir().unwrap();
    cli(dir.path())
        .args(["install", "--agent", "all"])
        .assert()
        .success();
    let claude = dir.path().join("claude/settings.json");
    let installed = fs::read(&claude).unwrap();
    let value: Value = serde_json::from_slice(&installed).unwrap();
    assert!(
        value["hooks"]["SubagentStart"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .ends_with(" hook claude")
    );
    assert!(value["hooks"]["SubagentStart"][0].get("matcher").is_none());
    let codex: Value =
        serde_json::from_slice(&fs::read(dir.path().join("codex/hooks.json")).unwrap()).unwrap();
    assert!(codex["hooks"].get("SubagentStart").is_none(), "{codex}");
    cli(dir.path())
        .args(["install", "--agent", "claude"])
        .assert()
        .success();
    assert_eq!(fs::read(&claude).unwrap(), installed);
    let doctor = |dir: &std::path::Path| -> Value {
        let out = cli(dir).arg("doctor").output().unwrap();
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["integration"]["hosts"][0].clone()
    };
    assert_eq!(doctor(dir.path())["hooks_configured"], true);
    // An installation from before SubagentStart is reported as incomplete.
    let mut older = value.clone();
    older["hooks"]
        .as_object_mut()
        .unwrap()
        .remove("SubagentStart");
    fs::write(&claude, serde_json::to_vec_pretty(&older).unwrap()).unwrap();
    assert_eq!(doctor(dir.path())["hooks_configured"], false);
    cli(dir.path())
        .args(["install", "--agent", "claude"])
        .assert()
        .success();
    cli(dir.path())
        .args(["uninstall", "--agent", "claude"])
        .assert()
        .success();
    let value: Value = serde_json::from_slice(&fs::read(&claude).unwrap()).unwrap();
    assert!(value.get("hooks").is_none(), "{value}");
}

/// `stats` reports per profile how often views were expanded and how, what
/// repeats saved, which views anchors shaped, and flags a profile whose
/// views are expanded too often; the journal holds no anchor term.
#[test]
fn stats_report_expand_kinds_repeats_anchors_and_tight_budgets() {
    let dir = tempfile::tempdir().unwrap();
    let words = |seed: usize| -> String {
        (0..300)
            .map(|i| {
                let w: String = (seed * 1000 + i)
                    .to_string()
                    .bytes()
                    .map(|b| (b'a' + (b - b'0')) as char)
                    .collect();
                format!("{w} test step done\n")
            })
            .collect()
    };
    let prompt = json!({"hook_event_name":"UserPromptSubmit","session_id":"st","prompt":"why does `zzanchor_term` break?"});
    hook_v4(dir.path(), "claude", prompt);
    let mut views = Vec::new();
    for seed in 0..4 {
        let mut out = words(seed + 1);
        if seed == 3 {
            out.insert_str(out.len() / 2, "bcd zzanchor_term step done\n");
        }
        let event = bash_result("st", "npm test", &out);
        let result = hook_v4(dir.path(), "claude", event.clone());
        views.push(shown(&event, &result, "stdout"));
    }
    // The first output again: a repeat reference.
    let again = bash_result("st", "npm test", &words(1));
    assert!(
        shown(
            &again,
            &hook_v4(dir.path(), "claude", again.clone()),
            "stdout"
        )
        .contains("repeat:")
    );
    let id = |view: &str| view[13..29].to_owned();
    cli(dir.path())
        .args(["expand", &id(&views[0]), "--find", "step"])
        .assert()
        .success();
    cli(dir.path())
        .args(["expand", &id(&views[1]), "--manifest"])
        .assert()
        .success();
    let out = cli(dir.path())
        .args(["stats", "--days", "1"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stats: Value = serde_json::from_slice(&out.stdout).unwrap();
    let tests = &stats["profiles"]["tests"];
    assert_eq!(tests["compressions"], 4, "{stats}");
    assert_eq!(tests["expanded"], 2, "{stats}");
    assert_eq!(tests["expand_rate"], 0.5, "{stats}");
    assert_eq!(tests["budget_probably_tight"], true, "{stats}");
    assert_eq!(tests["repeats"], 1, "{stats}");
    assert_eq!(tests["anchored"], 1, "{stats}");
    assert_eq!(stats["repeats"], 1, "{stats}");
    assert!(
        stats["repeat_bytes_saved"].as_u64().unwrap() > 5000,
        "{stats}"
    );
    assert_eq!(
        stats["expands_by_kind"],
        json!({"find": 1, "manifest": 1}),
        "{stats}"
    );
    assert!(stats["pass_through"].get("repeat").is_none(), "{stats}");
    let journal: String = fs::read_dir(dir.path().join("cache/events"))
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .map(|e| fs::read_to_string(e.path()).unwrap())
        .collect();
    assert!(!journal.contains("zzanchor"), "{journal}");
    assert!(journal.contains("\"anchored\":true"), "{journal}");
}
