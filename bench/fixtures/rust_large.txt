//! Host adapters and reversible installation. No model/network calls.
use crate::{
    compress,
    store::{MAX_INPUT, digest},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, clap::ValueEnum, PartialEq)]
pub enum Agent {
    Claude,
    Codex,
    Opencode,
    All,
}
const HOSTS: [Agent; 3] = [Agent::Claude, Agent::Codex, Agent::Opencode];
/// OpenCode loads every `plugin/*.js` under its config directory; this marker
/// is how install and uninstall recognise the file as ours.
const PLUGIN_MARKER: &str = "ScopeletPlugin";
const PLUGIN_TEMPLATE: &str = include_str!("opencode_plugin.js");
#[derive(Clone, Copy, Default, clap::ValueEnum, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Preference {
    #[default]
    Default,
    Caveman,
    Off,
}

fn root() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("SCOPELET_CONFIG_DIR") {
        return Ok(path.into());
    }
    let home = std::env::var_os("HOME").context("HOME is required")?;
    Ok(std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(home).join(".config"))
        .join("scopelet"))
}
fn private_dir(path: &Path) -> Result<()> {
    ensure!(
        !fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()),
        "directory is a symlink"
    );
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    Ok(())
}
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure!(
        !fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()),
        "refusing to replace symlink"
    );
    let parent = path.parent().context("path needs parent")?;
    private_dir(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    if let Ok(meta) = fs::metadata(path) {
        temp.as_file().set_permissions(meta.permissions())?;
    }
    temp.persist(path)?;
    Ok(())
}
fn preference() -> Result<Preference> {
    match fs::read(root()?.join("mode.json")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Preference::Default),
        Err(e) => Err(e.into()),
    }
}
pub fn set_mode(mode: Preference) -> Result<()> {
    write_atomic(&root()?.join("mode.json"), &serde_json::to_vec(&mode)?)
}
fn host_file(agent: Agent) -> Result<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME is required")?);
    Ok(match agent {
        Agent::Claude => std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".claude"))
            .join("settings.json"),
        Agent::Codex => std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"))
            .join("hooks.json"),
        // OpenCode honours OPENCODE_CONFIG_DIR, then the XDG config root.
        Agent::Opencode => std::env::var_os("OPENCODE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".config"))
                    .join("opencode")
            })
            .join("plugin")
            .join("scopelet.js"),
        Agent::All => anyhow::bail!("select one host"),
    })
}
fn host_name(agent: Agent) -> &'static str {
    match agent {
        Agent::Claude => "claude",
        Agent::Codex => "codex",
        Agent::Opencode => "opencode",
        Agent::All => "all",
    }
}
fn plugin_source(config: &Path, binary: &Path) -> Result<String> {
    Ok(PLUGIN_TEMPLATE
        .replace(
            "__SCOPELET_BINARY__",
            &serde_json::to_string(&binary.to_string_lossy())?,
        )
        .replace(
            "__SCOPELET_CONFIG__",
            &serde_json::to_string(&config.to_string_lossy())?,
        ))
}
fn backup(config: &Path, name: &str, bytes: &[u8], extension: &str) -> Result<()> {
    let path = config
        .join("backups")
        .join(format!("{name}-{}.{extension}", digest(bytes)));
    if !path.exists() {
        write_atomic(&path, bytes)?;
    }
    Ok(())
}
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}
fn owned(group: &Value, command: &str) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks.len() == 1 && hooks[0].get("command").and_then(Value::as_str) == Some(command)
        })
}
pub fn install(agent: Agent, remove: bool) -> Result<Value> {
    let config = root()?;
    let binary = config.join("bin/scopelet");
    if !remove {
        let bytes = fs::read(std::env::current_exe()?)?;
        if fs::read(&binary).ok().as_deref() != Some(&bytes) {
            write_atomic(&binary, &bytes)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))?;
        }
        if !config.join("mode.json").exists() {
            set_mode(Preference::Default)?;
        }
    }
    let agents = if agent == Agent::All {
        HOSTS.to_vec()
    } else {
        vec![agent]
    };
    let mut paths = Vec::new();
    for agent in agents {
        let path = host_file(agent)?;
        let old = match fs::read(&path) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let name = host_name(agent);
        if agent == Agent::Opencode {
            // A whole file rather than a JSON entry: never touch a plugin that
            // is not ours, and keep the same backup discipline as the others.
            ensure!(
                old.as_deref()
                    .is_none_or(|b| String::from_utf8_lossy(b).contains(PLUGIN_MARKER)),
                "refusing to replace {}: not a Scopelet plugin",
                path.display()
            );
            let rendered = plugin_source(&config, &binary)?;
            if remove {
                if let Some(bytes) = &old {
                    backup(&config, name, bytes, "js")?;
                    fs::remove_file(&path)?;
                }
            } else if old.as_deref() != Some(rendered.as_bytes()) {
                if let Some(bytes) = &old {
                    backup(&config, name, bytes, "js")?;
                }
                write_atomic(&path, rendered.as_bytes())?;
            }
            paths.push(path);
            continue;
        }
        let mut value: Value = match &old {
            Some(b) => serde_json::from_slice(b).context("invalid existing host configuration")?,
            None => json!({}),
        };
        let command = format!(
            "SCOPELET_CONFIG_DIR={} {} hook {name}",
            quote(&config.to_string_lossy()),
            quote(&binary.to_string_lossy())
        );
        let events = if agent == Agent::Codex {
            vec!["SessionStart", "UserPromptSubmit", "PreToolUse"]
        } else {
            vec!["SessionStart", "UserPromptSubmit", "PostToolUse"]
        };
        let object = value
            .as_object_mut()
            .context("host config must be an object")?;
        if remove && !object.contains_key("hooks") {
            continue;
        }
        let hooks = object
            .entry("hooks")
            .or_insert(json!({}))
            .as_object_mut()
            .context("hooks must be an object")?;
        for event in events {
            if remove && !hooks.contains_key(event) {
                continue;
            }
            let groups = hooks
                .entry(event)
                .or_insert(json!([]))
                .as_array_mut()
                .context("hook event must be an array")?;
            groups.retain(|group| !owned(group, &command));
            if !remove {
                let mut entry =
                    json!({"hooks":[{"type":"command", "command":command,"timeout":5}]});
                if event.ends_with("ToolUse") {
                    entry["matcher"] = json!("Bash");
                }
                groups.push(entry);
            }
            if groups.is_empty() {
                hooks.remove(event);
            }
        }
        if hooks.is_empty() {
            object.remove("hooks");
        }
        let updated = serde_json::to_vec_pretty(&value)?;
        // Compare semantic values, so reinstalling does not churn files/backups.
        if old
            .as_ref()
            .and_then(|b| serde_json::from_slice::<Value>(b).ok())
            .as_ref()
            != Some(&value)
        {
            if let Some(bytes) = &old {
                backup(&config, name, bytes, "json")?;
            }
            if old.is_some() || !remove {
                write_atomic(&path, &updated)?;
            }
        }
        paths.push(path);
    }
    Ok(
        json!({"installed":!remove,"files":paths,"binary":binary,"mode":preference()?,"note":"Codex: review new hooks with /hooks. Restart active sessions after installation."}),
    )
}
pub fn doctor() -> Value {
    let config = root().ok();
    let binary = config.as_ref().map(|p| p.join("bin/scopelet"));
    let files: Vec<_> = HOSTS.into_iter().map(|agent| {
        let path = host_file(agent).ok();
        let name = host_name(agent);
        let bytes = path.as_ref().and_then(|p| fs::read(p).ok());
        let configured = if agent == Agent::Opencode {
            // The plugin is ours and points at the binary this doctor knows.
            let rendered = config.as_ref().zip(binary.as_ref()).and_then(|(c, b)| plugin_source(c, b).ok());
            bytes.as_deref().zip(rendered.as_deref()).is_some_and(|(b, r)| b == r.as_bytes())
        } else {
            let event = if agent == Agent::Codex { "PreToolUse" } else { "PostToolUse" };
            let value = bytes.as_deref().and_then(|b| serde_json::from_slice::<Value>(b).ok());
            let command = config.as_ref().zip(binary.as_ref()).map(|(c,b)| format!("SCOPELET_CONFIG_DIR={} {} hook {name}", quote(&c.to_string_lossy()), quote(&b.to_string_lossy())));
            value.as_ref().zip(command.as_ref()).is_some_and(|(v,c)| {
                ["SessionStart", "UserPromptSubmit", event].iter().all(|e| v["hooks"][e].as_array().is_some_and(|groups| groups.iter().any(|group| owned(group,c))))
            })
        };
        json!({"host":name,"config":path,"hooks_configured":configured,"config_exists":path.as_ref().is_some_and(|p|p.is_file())})
    }).collect();
    json!({"mode":preference().ok(),"hosts":files,"binary_installed":binary.is_some_and(|p|p.is_file()),"automatic_coverage":"Claude Bash PostToolUse; Codex simple noninteractive Bash PreToolUse; OpenCode bash tool.execute.after plugin; hook trust and host versions must be verified"})
}

/// A deliberately small shell grammar. Any expansion, pipeline or control operator passes through.
fn simple_args(command: &str) -> Option<Vec<String>> {
    if command.contains([
        '\n', '\r', '$', '`', '|', '&', ';', '<', '>', '(', ')', '{', '}', '*', '?', '[', ']', '!',
        '\\',
    ]) {
        return None;
    }
    let mut args = Vec::new();
    let mut token = String::new();
    let mut quoted = None;
    let mut started = false;
    for c in command.chars() {
        match quoted {
            Some(q) if c == q => quoted = None,
            Some(_) => token.push(c),
            None if c == '\'' || c == '"' => {
                quoted = Some(c);
                started = true;
            }
            None if c.is_whitespace() => {
                if started {
                    args.push(std::mem::take(&mut token));
                    started = false;
                }
            }
            None => {
                token.push(c);
                started = true;
            }
        }
    }
    if quoted.is_some() {
        return None;
    }
    if started {
        args.push(token);
    }
    if args.is_empty() {
        return None;
    }
    Some(args)
}
/// A short AND-list of eligible argv commands has well-defined noninteractive
/// semantics. Reconstruct only that grammar; never reinterpret arbitrary shell.
fn command_args(command: &str) -> Option<Vec<String>> {
    if !command.contains("&&") {
        return simple_args(command).filter(|args| eligible(args));
    }
    let parts: Vec<_> = command.split("&&").collect();
    if parts.len() > 8 {
        return None;
    }
    let commands: Option<Vec<_>> = parts
        .iter()
        .map(|part| simple_args(part).filter(|args| eligible(args)))
        .collect();
    let script = commands?
        .iter()
        .map(|args| args.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join(" && ");
    Some(vec!["/bin/sh".into(), "-c".into(), script])
}

fn eligible(args: &[String]) -> bool {
    crate::commands::profile(args).is_some()
}
// This is only a compression bypass: native execution still reads the file,
// including changes after this metadata check, and enforces host permissions.
fn small_file_read(args: &[String], event: &Value) -> bool {
    if args.len() != 2 || Path::new(&args[0]).file_name().and_then(|n| n.to_str()) != Some("cat") {
        return false;
    }
    let file = Path::new(&args[1]);
    let path = if file.is_absolute() {
        file.to_path_buf()
    } else {
        let Some(cwd) = event["tool_input"]["workdir"]
            .as_str()
            .or_else(|| event["cwd"].as_str())
        else {
            return false;
        };
        if !Path::new(cwd).is_absolute() {
            return false;
        }
        Path::new(cwd).join(file)
    };
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() <= compress::SMALL as u64)
}

fn context(event: &Value, mode: Preference) -> Result<Value> {
    let name = event["hook_event_name"].as_str().unwrap_or("");
    let Some(session) = event["session_id"].as_str() else {
        return Ok(json!({}));
    };
    let state = root()?.join("sessions").join(digest(session.as_bytes()));
    let current = serde_json::to_vec(&mode)?;
    if name != "SessionStart" && fs::read(&state).ok().as_deref() == Some(&current) {
        return Ok(json!({}));
    }
    write_atomic(&state, &current)?;
    Ok(json!({"hookSpecificOutput":{"hookEventName":name,"additionalContext":guidance(mode)}}))
}
fn guidance(mode: Preference) -> &'static str {
    match mode {
        Preference::Default => {
            "Scopelet auto: for routine edits, inspect relevant code and existing checks; preserve their intended behavior and verify the change. Report outcome and validation in 1–3 short sentences. Expand for requested detail, uncertainty or next steps. Recover partial evidence when needed."
        }
        Preference::Caveman => {
            "Scopelet caveman: routine replies target 30 words, telegraphic, in the user's language. Preserve errors, qualifications, negation, numbers and next actions; exceed the target when needed or requested. Inspect relevant code and existing checks; verify intended behavior. Documents use normal prose. Recover partial evidence when needed."
        }
        Preference::Off => "Scopelet is off. Resume normal tools and response style.",
    }
}
/// OpenCode's plugin sends two events: a system-prompt pass (rebuilt on every
/// model step, so no per-session state) and one bash result to compress.
fn opencode(event: &Value, mode: Preference, version: compress::Version) -> Result<Value> {
    let name = event["hook_event_name"].as_str().unwrap_or("");
    if mode == Preference::Off {
        return Ok(json!({}));
    }
    if name == "SystemPrompt" {
        return Ok(json!({"system": guidance(mode)}));
    }
    if name != "ToolOutput" || event["tool_name"] != "Bash" {
        return Ok(json!({}));
    }
    if event["tool_input"]["command"]
        .as_str()
        .is_some_and(|s| s.contains("scopelet") || s.contains("rtk"))
    {
        return Ok(json!({}));
    }
    let Some(raw) = event["tool_response"]["output"].as_str() else {
        return Ok(json!({}));
    };
    if raw.len() <= compress::SMALL || raw.len() > MAX_INPUT {
        return Ok(json!({}));
    }
    let small = compress::automatic_lazy(raw.as_bytes(), None, 4096, version)?;
    if small.as_ref() == raw.as_bytes() {
        return Ok(json!({}));
    }
    Ok(json!({"output": String::from_utf8(small.into_owned())?}))
}
pub fn hook_stdin(agent: Agent) -> Result<Value> {
    hook_stdin_version(agent, compress::Version::configured(None)?)
}
pub fn hook_stdin_version(agent: Agent, version: compress::Version) -> Result<Value> {
    ensure!(agent != Agent::All, "hook needs one agent");
    let mut bytes = Vec::new();
    std::io::stdin()
        .take((MAX_INPUT * 3 + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_INPUT * 3, "hook input too large");
    let event: Value = serde_json::from_slice(&bytes)?;
    hook_version(agent, &event, version)
}
pub fn hook(agent: Agent, event: &Value) -> Result<Value> {
    hook_version(agent, event, compress::Version::configured(None)?)
}
fn hook_version(agent: Agent, event: &Value, version: compress::Version) -> Result<Value> {
    let mode = preference()?;
    if agent == Agent::Opencode {
        return opencode(event, mode, version);
    }
    let name = event["hook_event_name"].as_str().unwrap_or("");
    if matches!(name, "SessionStart" | "UserPromptSubmit") {
        return context(event, mode);
    }
    if mode == Preference::Off || event["tool_name"] != "Bash" {
        return Ok(json!({}));
    }
    if agent == Agent::Codex && name == "PreToolUse" {
        if event["tool_input"]["tty"] == true || event["tool_input"]["run_in_background"] == true {
            return Ok(json!({}));
        }
        let Some(command) = event["tool_input"]["command"].as_str() else {
            return Ok(json!({}));
        };
        let Some(args) = command_args(command) else {
            return Ok(json!({}));
        };
        if small_file_read(&args, event) {
            return Ok(json!({}));
        }
        let binary = std::env::current_exe()?;
        let command = format!(
            "{} run --auto --timeout 3600 {} -- {}",
            quote(&binary.to_string_lossy()),
            match version {
                compress::Version::V1 => "--compact-version 1",
                compress::Version::V2 => "--compact-version 2",
                compress::Version::V3 => "--compact-version 3",
            },
            args.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" ")
        );
        let mut input = event["tool_input"].clone();
        input["command"] = json!(command);
        return Ok(
            json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":input}}),
        );
    }
    if agent == Agent::Claude && name == "PostToolUse" {
        if event["tool_input"]["command"]
            .as_str()
            .is_some_and(|s| s.contains("scopelet") || s.contains("rtk"))
        {
            return Ok(json!({}));
        }
        let mut output = event["tool_response"].clone();
        if output["isImage"] == true
            || !output["stdout"].is_string()
            || !output["stderr"].is_string()
        {
            return Ok(json!({}));
        }
        // Claude can attach persistence metadata before rendering its preview.
        // Replacing stdout here would make that preview truncate our compact
        // view a second time, hiding its final diagnostics and omission footer.
        if output["persistedOutputPath"]
            .as_str()
            .is_some_and(|path| !path.is_empty())
            || output["persistedOutputSize"]
                .as_u64()
                .is_some_and(|size| size > 0)
        {
            return Ok(json!({}));
        }
        if ["stdout", "stderr"].iter().all(|stream| {
            output[stream]
                .as_str()
                .is_some_and(|text| text.len() <= compress::SMALL)
        }) {
            return Ok(json!({}));
        }
        let mut changed = false;
        for stream in ["stdout", "stderr"] {
            let raw = output[stream].as_str().unwrap();
            if raw.len() > MAX_INPUT {
                return Ok(json!({}));
            }
            let small = compress::automatic_lazy(raw.as_bytes(), None, 4096, version)?;
            if small.as_ref() != raw.as_bytes() {
                output[stream] = json!(String::from_utf8(small.into_owned())?);
                changed = true;
            }
        }
        if changed {
            return Ok(
                json!({"hookSpecificOutput":{"hookEventName":"PostToolUse","updatedToolOutput":output}}),
            );
        }
    }
    Ok(json!({}))
}
