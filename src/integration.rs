//! Host adapters and reversible installation. No model/network calls.
use crate::{
    commands::Profile,
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
#[derive(Clone, Copy, Debug, Default, clap::ValueEnum, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Preference {
    #[default]
    Default,
    Caveman,
    Off,
}

pub(crate) fn root() -> Result<PathBuf> {
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
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
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
        )
        .replace("__SCOPELET_SMALL__", &compress::SMALL.to_string())
        .replace("__SCOPELET_MAX_INPUT__", &MAX_INPUT.to_string()))
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
            vec![
                "SessionStart",
                "UserPromptSubmit",
                "PostToolUse",
                "SubagentStart",
            ]
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
                if event == "PostToolUse" {
                    // Claude Code: Bash output, and Grep's text in content mode.
                    entry["matcher"] = json!("Bash|Grep");
                } else if event.ends_with("ToolUse") {
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
    let note = if remove {
        "Restart active sessions so they stop running the removed hooks. The pinned binary, mode and backups stay in the Scopelet config directory."
    } else {
        "Codex: review new hooks with /hooks. Restart active sessions after installation."
    };
    Ok(json!({"installed":!remove,"files":paths,"binary":binary,"mode":preference()?,"note":note}))
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
                let mut events = vec!["SessionStart", "UserPromptSubmit", event];
                if agent == Agent::Claude {
                    events.push("SubagentStart");
                }
                events.iter().all(|e| v["hooks"][e].as_array().is_some_and(|groups| groups.iter().any(|group| owned(group,c))))
            })
        };
        json!({"host":name,"config":path,"hooks_configured":configured,"config_exists":path.as_ref().is_some_and(|p|p.is_file())})
    }).collect();
    let mut status = json!({"mode":preference().ok(),"hosts":files,"binary_installed":binary.as_ref().is_some_and(|p|p.is_file()),"automatic_coverage":"Claude Bash and Grep (content mode) PostToolUse; Codex recognized Bash commands in a quoted shell subset, PreToolUse; OpenCode bash tool.execute.after plugin; hook trust and host versions must be verified"});
    if let Some(binary) = binary.filter(|p| p.is_file()) {
        let (current, version) = pinned_binary(&binary);
        status["binary"] = json!(binary);
        status["binary_version"] = json!(version);
        status["running_version"] = json!(format!("scopelet {}", env!("CARGO_PKG_VERSION")));
        status["binary_current"] = json!(current);
        if current == Some(false) {
            status["binary_note"] = json!(
                "Hooks run an older copy than this scopelet; run `scopelet install --agent all` and restart sessions."
            );
        }
    }
    status
}

/// Whether the pinned hook binary is byte for byte the running one (None
/// when either cannot be read), and the version it reports.
fn pinned_binary(binary: &Path) -> (Option<bool>, Option<String>) {
    let pinned = fs::read(binary).ok();
    let running = std::env::current_exe().ok().and_then(|p| fs::read(p).ok());
    let current = pinned.zip(running).map(|(a, b)| a == b);
    let version = crate::process::capture(
        std::process::Command::new(binary).arg("--version"),
        std::time::Duration::from_secs(5),
        4096,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .ok()
    .filter(|result| crate::process::exit_code(result) == 0)
    .map(|result| String::from_utf8_lossy(&result.stdout).trim().to_owned());
    (current, version)
}

/// The argv `run --auto` executes for a command the hook may wrap, or None to
/// leave it to the host. The command must parse in the shell subset of
/// `shell`, every command in it must be a recognized noninteractive one (or
/// `cd DIR &&`, `|| true`, or a read-only filter after a pipe), and its
/// output must not already be bounded by a final `head`, `tail`, `wc` or
/// `grep -c`/`-l`/`-q`.
fn command_args(command: &str, version: compress::Version) -> Option<(Vec<String>, Profile)> {
    use crate::shell::{Joint, Plan};
    let script = crate::shell::parse(command)?;
    let last = script.steps.len() - 1;
    // The profile of the first recognized command sets the output budget.
    let mut recognized: Option<Profile> = None;
    for (i, (joint, pipeline)) in script.steps.iter().enumerate() {
        let first = &pipeline[0];
        if *joint == Joint::OrTrue {
            continue; // `|| true`, checked by the parser
        }
        if first.argv[0] == "cd" && pipeline.len() == 1 {
            // `cd DIR &&` only: a directory change that leads somewhere.
            if first.argv.len() != 2 || !first.env.is_empty() || i == last {
                return None;
            }
            if script
                .steps
                .get(i + 1)
                .is_none_or(|(j, _)| *j != Joint::And)
            {
                return None;
            }
            continue;
        }
        let profile = crate::commands::profile_for(&first.argv, version == compress::Version::V4)?;
        if !first
            .env
            .iter()
            .all(|(name, _)| crate::commands::safe_env(name))
        {
            return None;
        }
        recognized.get_or_insert(profile);
        for filter in &pipeline[1..] {
            if !filter.env.is_empty() {
                return None;
            }
            crate::commands::filter(&filter.argv)?;
        }
        if pipeline.len() > 1
            && crate::commands::filter(&pipeline.last().unwrap().argv)?.bounds_output
        {
            return None;
        }
    }
    let profile = recognized?;
    let argv = match script.plan() {
        Plan::Direct(argv) => argv,
        Plan::AndList(commands) => {
            let script = commands
                .iter()
                .map(|args| args.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" "))
                .collect::<Vec<_>>()
                .join(" && ");
            vec!["/bin/sh".into(), "-c".into(), script]
        }
        Plan::Shell => vec!["/bin/sh".into(), "-c".into(), command.into()],
    };
    Some((argv, profile))
}
// This is only a compression bypass: native execution still reads the file,
// including changes after this metadata check, and enforces host permissions.
/// `small` is the exact-size threshold of the command's profile.
fn small_file_read(args: &[String], small: usize, event: &Value) -> bool {
    let reader = Path::new(&args[0]).file_name().and_then(|n| n.to_str());
    if args.len() != 2 || !matches!(reader, Some("cat" | "nl")) {
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
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() <= small as u64)
}

/// Remove per-session context markers older than `older_days`; how many.
pub fn clean_sessions(older_days: u64) -> Result<usize> {
    let folder = root()?.join("sessions");
    let age = std::time::Duration::from_secs(older_days.saturating_mul(86400));
    let now = std::time::SystemTime::now();
    let mut removed = 0;
    let entries = match fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let ours = name
            .to_str()
            .is_some_and(|n| n.len() == 64 && n.bytes().all(|b| b.is_ascii_hexdigit()));
        let metadata = entry.metadata()?;
        if ours
            && metadata.is_file()
            && now.duration_since(metadata.modified()?).unwrap_or_default() >= age
        {
            fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// The private state key of the context an event belongs to: its session,
/// and its subagent when the host names one.
fn context_key(event: &Value) -> Option<String> {
    let session = event["session_id"].as_str().filter(|s| !s.is_empty())?;
    Some(crate::session::key(session, event["agent_id"].as_str()))
}

/// What a Claude Code tool result offers for compression: the string fields
/// that carry its text, the profile that shapes them and the size up to which
/// they stay exact, the command its anchors come from and extra anchors.
struct ClaudeResult<'a> {
    fields: &'static [&'static str],
    profile: Option<Profile>,
    small: usize,
    command: &'a str,
    anchors: Vec<String>,
}

/// The fields of a Claude Code result to compress, or None for a shape left
/// alone: Bash's `stdout` and `stderr`; Grep's `content` in `content` mode,
/// as search results (its `files_with_matches` and `count` modes, and any
/// unknown shape, pass through).
fn claude_fields<'a>(event: &'a Value, version: compress::Version) -> Option<ClaudeResult<'a>> {
    let output = &event["tool_response"];
    if event["tool_name"] == "Grep" {
        if output["mode"] != "content" || !output["content"].is_string() {
            return None;
        }
        let profile = compress::effective_profile(Some(Profile::Search), version);
        let anchors = event["tool_input"]["pattern"]
            .as_str()
            .map(crate::commands::pattern_anchors)
            .unwrap_or_default();
        return Some(ClaudeResult {
            fields: &["content"],
            profile,
            small: compress::SMALL,
            command: "",
            anchors,
        });
    }
    let command = event["tool_input"]["command"].as_str().unwrap_or("");
    if crate::commands::invokes_wrapper(command)
        || output["isImage"] == true
        || !output["stdout"].is_string()
        || !output["stderr"].is_string()
    {
        return None;
    }
    let (profile, small) = limits(command, version);
    Some(ClaudeResult {
        fields: &["stdout", "stderr"],
        profile,
        small,
        command,
        anchors: Vec::new(),
    })
}

/// Claude Code's PostToolUse: replace the text fields of a Bash or Grep
/// result by their views, keeping every other field as it was.
fn claude_result(event: &Value, version: compress::Version) -> Result<Value> {
    let Some(result) = claude_fields(event, version) else {
        return Ok(json!({}));
    };
    let ClaudeResult {
        fields,
        profile,
        small,
        command,
        anchors: extra,
    } = result;
    let mut output = event["tool_response"].clone();
    let total: usize = fields
        .iter()
        .map(|f| output[*f].as_str().map_or(0, str::len))
        .sum();
    let unchanged = |reason| crate::events::unchanged("claude", profile, version, total, reason);
    // Claude can attach persistence metadata before rendering its preview.
    // Replacing stdout here would make that preview truncate our compact
    // view a second time, hiding its final diagnostics and omission footer.
    // The model only saw a preview of a persisted output: it is neither
    // compressed nor remembered as shown.
    if output["persistedOutputPath"]
        .as_str()
        .is_some_and(|path| !path.is_empty())
        || output["persistedOutputSize"]
            .as_u64()
            .is_some_and(|size| size > 0)
    {
        unchanged(compress::Reason::Persisted);
        return Ok(json!({}));
    }
    let mut changed = false;
    // Compact-v4 replaces a field identical to one this context was already
    // shown (whole, as a view or raw) by a reference to it.
    let mut repeated = vec![false; fields.len()];
    let context = context_key(event).filter(|_| version == compress::Version::V4);
    let mut state = context.as_deref().map(crate::session::load);
    if let Some(state) = &mut state {
        for (i, field) in fields.iter().enumerate() {
            let raw = output[*field].as_str().unwrap();
            if raw.len() <= compress::SMALL || raw.len() > MAX_INPUT {
                continue;
            }
            let hash = digest(raw.as_bytes());
            let hash = crate::session::short(&hash);
            if state.saw(hash)
                && let Some(marker) =
                    compress::repeat_recorded("claude", raw.as_bytes(), None, profile)
            {
                output[*field] = json!(String::from_utf8(marker)?);
                repeated[i] = true;
                changed = true;
            }
            state.remember(hash);
        }
    }
    let anchors = if version == compress::Version::V4 {
        let mut terms = anchors(event, state.as_ref(), command);
        terms.extend(extra);
        terms
    } else {
        Vec::new()
    };
    let remembered = || {
        if let (Some(key), Some(state)) = (&context, &state) {
            // Best effort: a lost write only misses a later repeat.
            let _ = crate::session::save(key, state);
        }
    };
    if !changed
        && fields.iter().all(|field| {
            output[*field]
                .as_str()
                .is_some_and(|text| text.len() <= small)
        })
    {
        unchanged(compress::Reason::Small);
        remembered();
        return Ok(json!({}));
    }
    for (i, field) in fields.iter().enumerate() {
        if repeated[i] {
            continue;
        }
        let raw = output[*field].as_str().unwrap();
        if raw.len() > MAX_INPUT {
            return Ok(json!({}));
        }
        let small = compress::automatic_anchored(
            "claude",
            raw.as_bytes(),
            None,
            profile,
            None,
            version,
            &anchors,
        );
        if small.as_ref() != raw.as_bytes() {
            output[*field] = json!(String::from_utf8(small.into_owned())?);
            changed = true;
        }
    }
    remembered();
    if changed {
        return Ok(
            json!({"hookSpecificOutput":{"hookEventName":"PostToolUse","updatedToolOutput":output}}),
        );
    }
    Ok(json!({}))
}

/// The relevance anchors of a tool call: the terms of the context's prompts
/// (a subagent also gets its session's, whose task it works on) and those
/// its command names.
fn anchors(event: &Value, state: Option<&crate::session::State>, command: &str) -> Vec<String> {
    let mut terms: Vec<String> = state.map(|s| s.anchors.clone()).unwrap_or_default();
    if event["agent_id"].as_str().is_some_and(|a| !a.is_empty())
        && let Some(session) = event["session_id"].as_str()
    {
        terms.extend(crate::session::load(&crate::session::key(session, None)).anchors);
    }
    terms.extend(crate::commands::anchors(command));
    terms
}

/// Guidance at a session start or a mode change. A compaction or a clear
/// starts the context over: what it was shown is no longer in it.
fn context(event: &Value, mode: Preference) -> Result<Value> {
    let name = event["hook_event_name"].as_str().unwrap_or("");
    let Some(key) = context_key(event) else {
        return Ok(json!({}));
    };
    let mut state = crate::session::load(&key);
    let before = state.clone();
    let start = name == "SessionStart";
    if start && matches!(event["source"].as_str(), Some("compact" | "clear")) {
        state.seen.clear();
    }
    if start && event["source"] == "clear" {
        state.anchors.clear();
    }
    // Prompt terms stay in this private state, never in the event journal.
    if name == "UserPromptSubmit"
        && let Some(prompt) = event["prompt"].as_str()
    {
        state.anchor(crate::session::prompt_anchors(prompt));
    }
    let announce = start || state.mode != Some(mode);
    state.mode = Some(mode);
    // A start rewrites the file so cleanup sees the context as recent.
    if start || state != before {
        crate::session::save(&key, &state)?;
    }
    if !announce {
        return Ok(json!({}));
    }
    Ok(json!({"hookSpecificOutput":{"hookEventName":name,"additionalContext":guidance(mode)}}))
}
/// How to recover what a view leaves out, with the pinned binary, which
/// works whether or not `scopelet` is on the PATH.
fn recover_line() -> String {
    let binary = root().map_or_else(
        |_| "scopelet".to_owned(),
        |root| quote(&root.join("bin/scopelet").to_string_lossy()),
    );
    format!("Recover: {binary} expand ID --find TEXT [--regex] [--ignore-case] (ID or last)")
}

/// Asks for less code before any is written: the session-level saving that
/// response style alone does not bring.
const REUSE: &str = "Before writing code: reuse existing code, stdlib or installed deps; make the smallest change that passes existing checks.";

fn guidance(mode: Preference) -> String {
    let recover = recover_line();
    match mode {
        Preference::Default => format!(
            "Scopelet auto: inspect relevant code and checks; keep behavior; verify. Report outcome and checks in 1-3 sentences; more if asked or unsure. {REUSE} {recover}"
        ),
        Preference::Caveman => format!(
            "Scopelet caveman: routine replies ~30 words, telegraphic, user's language; keep errors, caveats, negation, numbers, next steps; exceed if needed or asked. Verify changes. Docs: normal prose. {REUSE} {recover}"
        ),
        Preference::Off => "Scopelet is off. Resume normal tools and response style.".into(),
    }
}

/// What a subagent receives at its start: how to recover evidence, and a
/// report that its parent reads as input, so no command or raw output is
/// restated.
fn subagent_guidance() -> String {
    format!(
        "{} Final report: conclusions with file:line evidence; do not restate commands or raw output.",
        recover_line()
    )
}
/// The recognized profile of a command line and the size up to which its
/// output stays exact: the profile's in compact-v3, the default otherwise.
fn limits(command: &str, version: compress::Version) -> (Option<Profile>, usize) {
    let profile = compress::effective_profile(
        crate::commands::classify_for(command, version == compress::Version::V4),
        version,
    );
    (profile, profile.map_or(compress::SMALL, Profile::small))
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
    let command = event["tool_input"]["command"].as_str().unwrap_or("");
    if crate::commands::invokes_wrapper(command) {
        return Ok(json!({}));
    }
    let Some(raw) = event["tool_response"]["output"].as_str() else {
        return Ok(json!({}));
    };
    let (profile, small) = limits(command, version);
    if raw.len() <= small {
        crate::events::unchanged(
            "opencode",
            profile,
            version,
            raw.len(),
            compress::Reason::Small,
        );
        return Ok(json!({}));
    }
    if raw.len() > MAX_INPUT {
        return Ok(json!({}));
    }
    let state = context_key(event).map(|key| crate::session::load(&key));
    let anchors = anchors(event, state.as_ref(), command);
    let small = compress::automatic_anchored(
        "opencode",
        raw.as_bytes(),
        None,
        profile,
        None,
        version,
        &anchors,
    );
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
    if name == "SubagentStart" {
        if mode == Preference::Off {
            return Ok(json!({}));
        }
        return Ok(
            json!({"hookSpecificOutput":{"hookEventName":"SubagentStart","additionalContext":subagent_guidance()}}),
        );
    }
    let claude_grep = agent == Agent::Claude && event["tool_name"] == "Grep";
    if mode == Preference::Off || (event["tool_name"] != "Bash" && !claude_grep) {
        return Ok(json!({}));
    }
    if agent == Agent::Codex && name == "PreToolUse" {
        if event["tool_input"]["tty"] == true || event["tool_input"]["run_in_background"] == true {
            return Ok(json!({}));
        }
        let Some(command) = event["tool_input"]["command"].as_str() else {
            return Ok(json!({}));
        };
        let Some((args, profile)) = command_args(command, version) else {
            return Ok(json!({}));
        };
        let small = compress::effective_profile(Some(profile), version)
            .map_or(compress::SMALL, Profile::small);
        if small_file_read(&args, small, event) {
            return Ok(json!({}));
        }
        let binary = std::env::current_exe()?;
        // The context key lets the wrapped run read this context's anchors.
        let session = context_key(event).map_or(String::new(), |key| {
            format!("--session {} ", crate::session::short(&key))
        });
        let command = format!(
            "{} run --auto --timeout 3600 {session}{} -- {}",
            quote(&binary.to_string_lossy()),
            match compress::effective_profile(Some(profile), version) {
                // Each profile has its own budget, in v3 only.
                Some(profile) => format!(
                    "--compact-version {} --profile {}",
                    version.number(),
                    profile.name()
                ),
                None => format!("--compact-version {}", version.number()),
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
        return claude_result(event, version);
    }
    Ok(json!({}))
}
