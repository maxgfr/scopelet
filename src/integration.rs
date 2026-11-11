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
/// OpenCode loads every `plugins/*.js` under its config directory; this marker
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
            .join("plugins")
            .join("scopelet.js"),
        Agent::All => anyhow::bail!("select one host"),
    })
}
/// The file a host's configuration path leads to. A settings file, or the
/// directory holding it, kept in a dotfiles repository is a link: the file
/// it points at is the one to edit, and the link stays a link. The deepest
/// part of the path that exists is resolved; a dangling link resolves to
/// itself and is then refused like any other link.
fn resolved(path: &Path) -> PathBuf {
    let mut missing = Vec::new();
    let mut existing = path;
    loop {
        if let Ok(real) = fs::canonicalize(existing) {
            return missing
                .iter()
                .rev()
                .fold(real, |real, name| real.join(name));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name);
                existing = parent;
            }
            _ => return path.to_owned(),
        }
    }
}
/// The file a host names and the one to edit. A settings file is followed
/// through its links. The OpenCode plugin is generated, not edited: only its
/// directory is followed, so uninstalling a linked plugin removes the link,
/// never what it points at.
fn host_path(agent: Agent) -> Result<(PathBuf, PathBuf)> {
    let named = host_file(agent)?;
    let path = match (agent, named.parent(), named.file_name()) {
        (Agent::Opencode, Some(parent), Some(file)) => resolved(parent).join(file),
        _ => resolved(&named),
    };
    Ok((named, path))
}
/// Whether installing into this host's file would be refused.
fn validate(agent: Agent, path: &Path, remove: bool) -> Result<()> {
    // What writing would refuse, refused before anything is written: a link
    // that leads nowhere at the file (a live one was followed by `host_path`;
    // the OpenCode plugin's own link is replaced, not written through) or at
    // any directory on the way to it that does not exist yet. Uninstalling
    // writes nothing through such a link, so it is not stopped by one.
    let link = |p: &Path| fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink());
    if !remove {
        ensure!(
            agent == Agent::Opencode || !link(path),
            "refusing to replace symlink: {}",
            path.display()
        );
        for directory in path.ancestors().skip(1) {
            if fs::metadata(directory).is_ok() {
                break;
            }
            ensure!(
                !link(directory),
                "directory is a symlink: {}",
                directory.display()
            );
        }
    }
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if agent == Agent::Opencode {
        ensure!(
            String::from_utf8_lossy(&bytes).contains(PLUGIN_MARKER),
            "refusing to replace {}: not a Scopelet plugin",
            path.display()
        );
        return Ok(());
    }
    let value: Value =
        serde_json::from_slice(&bytes).context("invalid existing host configuration")?;
    let object = value.as_object().context("host config must be an object")?;
    // Only the events Scopelet edits must be arrays; others are someone
    // else's and are left as they are.
    if let Some(hooks) = object.get("hooks") {
        let hooks = hooks.as_object().context("hooks must be an object")?;
        ensure!(
            events(agent)
                .iter()
                .all(|event| hooks.get(*event).is_none_or(Value::is_array)),
            "hook event must be an array"
        );
    }
    Ok(())
}
/// The hook events Scopelet installs for a host.
fn events(agent: Agent) -> &'static [&'static str] {
    if agent == Agent::Codex {
        &["SessionStart", "UserPromptSubmit", "PreToolUse"]
    } else {
        &[
            "SessionStart",
            "UserPromptSubmit",
            "PostToolUse",
            "PostToolUseFailure",
            "SubagentStart",
        ]
    }
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
/// A hook group written by an install of Scopelet for this host, under any
/// configuration directory or binary path.
fn installed(group: &Value, host: &str) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks.len() == 1
                && hooks[0]
                    .get("command")
                    .and_then(Value::as_str)
                    .and_then(crate::commands::installed_hook)
                    .as_deref()
                    == Some(host)
        })
}
pub fn install(agent: Agent, remove: bool) -> Result<Value> {
    let config = root()?;
    let binary = config.join("bin/scopelet");
    let agents = if agent == Agent::All {
        HOSTS.to_vec()
    } else {
        vec![agent]
    };
    // Every host's file is checked before anything is written, the pinned
    // binary included: one that would be refused must not leave the hosts
    // before it, or the binary, half installed.
    for &agent in &agents {
        validate(agent, &host_path(agent)?.1, remove)?;
    }
    // Backups are written before a host's file is replaced: a directory for
    // them that is a link would be refused after the first host was written.
    let backups = config.join("backups");
    ensure!(
        !fs::symlink_metadata(&backups).is_ok_and(|m| m.file_type().is_symlink()),
        "directory is a symlink: {}",
        backups.display()
    );
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
    let mut paths = Vec::new();
    for agent in agents {
        // Reported as the host names it, edited where it really is.
        let (named, path) = host_path(agent)?;
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
            // Earlier releases wrote `plugin/scopelet.js`, which OpenCode still
            // loads: drop our copy so the hooks never run twice. It sits in
            // the configuration directory OpenCode names, wherever its
            // `plugins` directory leads.
            if let Some(legacy) = named.parent().and_then(Path::parent) {
                let legacy = resolved(legacy).join("plugin").join("scopelet.js");
                // `plugin` may lead to `plugins` itself: that is the file
                // being installed, not a second copy.
                let directory = |p: &Path| p.parent().and_then(|d| fs::canonicalize(d).ok());
                let same = directory(&legacy).is_some() && directory(&legacy) == directory(&path);
                if !same
                    && let Ok(bytes) = fs::read(&legacy)
                    && String::from_utf8_lossy(&bytes).contains(PLUGIN_MARKER)
                {
                    backup(&config, name, &bytes, "js")?;
                    fs::remove_file(&legacy)?;
                }
            }
            // What the plugin path holds now: a link to the legacy file just
            // removed leads nowhere, and must be written, not taken as current.
            let old = fs::read(&path).ok();
            let rendered = plugin_source(&config, &binary)?;
            if remove {
                if let Some(bytes) = &old {
                    backup(&config, name, bytes, "js")?;
                }
                // A link, even one the legacy cleanup just left dangling, is
                // ours to remove with the plugin.
                if fs::symlink_metadata(&path).is_ok() {
                    fs::remove_file(&path)?;
                }
            } else if old.as_deref() != Some(rendered.as_bytes()) {
                if let Some(bytes) = &old {
                    backup(&config, name, bytes, "js")?;
                }
                // A link to an earlier copy of the plugin is replaced by the
                // file itself; what it pointed at is left as it was.
                if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
                    fs::remove_file(&path)?;
                }
                write_atomic(&path, rendered.as_bytes())?;
            }
            paths.push(named);
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
        let events = events(agent);
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
        for &event in events {
            if remove && !hooks.contains_key(event) {
                continue;
            }
            let groups = hooks
                .entry(event)
                .or_insert(json!([]))
                .as_array_mut()
                .context("hook event must be an array")?;
            // Ours, whatever configuration directory or binary path an
            // earlier install wrote: a second set would run every hook twice.
            groups.retain(|group| !owned(group, &command) && !installed(group, name));
            if !remove {
                let mut entry =
                    json!({"hooks":[{"type":"command", "command":command,"timeout":5}]});
                if event == "PostToolUse" {
                    // Claude Code: Bash output, and Grep's text in content mode.
                    entry["matcher"] = json!("Bash|Grep");
                } else if event.ends_with("ToolUse") || event == "PostToolUseFailure" {
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
        // The user's file: its keys keep the order they were written in.
        let updated = crate::json_order::pretty_like(&value, old.as_deref());
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
        paths.push(named);
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
                    events.extend(["PostToolUseFailure", "SubagentStart"]);
                }
                events.iter().all(|e| v["hooks"][e].as_array().is_some_and(|groups| groups.iter().any(|group| owned(group,c))))
            })
        };
        let mut entry = json!({"host":name,"config":path,"hooks_configured":configured,"config_exists":path.as_ref().is_some_and(|p|p.is_file())});
        // Only a host set up here is asked for its version.
        if path.as_ref().is_some_and(|p| p.is_file()) {
            host_version(name, &mut entry);
        }
        entry
    }).collect();
    let mut status = json!({"mode":preference().ok(),"hosts":files,"binary_installed":binary.as_ref().is_some_and(|p|p.is_file()),"automatic_coverage":"Claude Bash and Grep (content mode) PostToolUse; Codex recognized Bash commands in a quoted shell subset, PreToolUse; OpenCode 2 shell execute.after plugin; hook trust and host versions must be verified"});
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
        // The pinned copy can be run as doctor itself: compare it with the
        // scopelet on the PATH, which a package manager upgrades.
        if let Some(path) = path_binary(&binary) {
            let path_version = version_of(&path, 5);
            status["path_binary"] = json!(path);
            status["path_binary_version"] = json!(path_version);
            status["pinned_outdated"] = json!(
                version
                    .as_deref()
                    .and_then(version_numbers)
                    .zip(path_version.as_deref().and_then(version_numbers))
                    .map(|(pinned, path)| pinned < path)
            );
        }
    }
    status
}

/// The host version on which each adapter's event shapes were last verified
/// (design.md): a newer host may have changed them.
const VERIFIED_HOSTS: [(&str, &str); 3] = [
    ("claude", "2.1.318"),
    ("codex", "0.160.0"),
    ("opencode", "2.0.0"),
];

/// A host's installed version (`host_version`, null when it does not run)
/// next to the one its adapter was verified on (`verified_with`), compared
/// as `same`, `newer` or `older` (`version_check`). Informative only: a
/// version never fails doctor.
fn host_version(host: &str, entry: &mut Value) {
    let Some((_, verified)) = VERIFIED_HOSTS.iter().find(|(name, _)| *name == host) else {
        return;
    };
    let version = version_of(Path::new(host), 5).and_then(|line| {
        line.split(|c: char| c.is_whitespace() || c == 'v')
            .find(|word| version_numbers(word).is_some_and(|n| n.len() >= 2))
            .map(str::to_owned)
    });
    let check = version
        .as_deref()
        .and_then(version_numbers)
        .zip(version_numbers(verified))
        .map(|(found, verified)| match found.cmp(&verified) {
            std::cmp::Ordering::Equal => "same",
            std::cmp::Ordering::Greater => "newer",
            std::cmp::Ordering::Less => "older",
        });
    entry["host_version"] = json!(version);
    entry["verified_with"] = json!(verified);
    entry["version_check"] = json!(check);
}

/// A version's numbers, from `1.2.3` or a `scopelet 1.2.3` line; None for
/// anything else (a pre-release suffix included).
fn version_numbers(text: &str) -> Option<Vec<u64>> {
    let text = text.trim();
    let text = text.strip_prefix("scopelet ").unwrap_or(text);
    text.split('.').map(|part| part.parse().ok()).collect()
}

/// The first line a program prints for `--version`, or None when it cannot
/// run, fails or takes longer than `seconds`.
fn version_of(program: &Path, seconds: u64) -> Option<String> {
    crate::process::capture(
        std::process::Command::new(program).arg("--version"),
        std::time::Duration::from_secs(seconds),
        4096,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .ok()
    .filter(|result| crate::process::exit_code(result) == 0)
    .and_then(|result| {
        String::from_utf8_lossy(&result.stdout)
            .lines()
            .next()
            .map(|line| line.trim().to_owned())
    })
}

/// The first `scopelet` on the PATH, unless it is the pinned copy itself.
/// Relative entries (`.`, an empty one) are skipped: they resolve against the
/// working directory, a project whose files are never run to ask a version.
fn path_binary(pinned: &Path) -> Option<PathBuf> {
    let found = std::env::split_paths(&std::env::var_os("PATH")?)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("scopelet"))
        .find(|path| path.is_file())?;
    (fs::canonicalize(&found).ok()? != fs::canonicalize(pinned).ok()?).then_some(found)
}

/// What a session start last learned about the scopelet on the PATH, so its
/// version is asked again only when that file changes.
#[derive(Deserialize, Serialize)]
struct PathCheck {
    path: PathBuf,
    len: u64,
    modified: u64,
    version: Option<String>,
}

/// The version of the scopelet on the PATH when the pinned hook binary that
/// is running is older: a package manager upgraded it and `install` was not
/// run again. `--version` runs only when the PATH file changed since the
/// last check, recorded in `<config>/path-binary.json`; any error is None.
fn newer_on_path() -> Option<String> {
    let config = root().ok()?;
    let pinned = config.join("bin/scopelet");
    let running = fs::canonicalize(std::env::current_exe().ok()?).ok()?;
    if running != fs::canonicalize(&pinned).ok()? {
        return None;
    }
    let path = path_binary(&pinned)?;
    let meta = fs::metadata(&path).ok()?;
    let modified = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let record = config.join("path-binary.json");
    let cached = fs::read(&record)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<PathCheck>(&bytes).ok())
        .filter(|c| c.path == path && c.len == meta.len() && c.modified == modified);
    let version = match cached {
        Some(check) => check.version,
        None => {
            let version = version_of(&path, 2);
            let check = PathCheck {
                path,
                len: meta.len(),
                modified,
                version: version.clone(),
            };
            if let Ok(bytes) = serde_json::to_vec(&check) {
                let _ = write_atomic(&record, &bytes);
            }
            version
        }
    }?;
    (version_numbers(&version)? > version_numbers(env!("CARGO_PKG_VERSION"))?).then_some(version)
}

/// Whether the pinned hook binary is byte for byte the running one (None
/// when either cannot be read), and the version it reports.
fn pinned_binary(binary: &Path) -> (Option<bool>, Option<String>) {
    let pinned = fs::read(binary).ok();
    let running = std::env::current_exe().ok().and_then(|p| fs::read(p).ok());
    let current = pinned.zip(running).map(|(a, b)| a == b);
    (current, version_of(binary, 5))
}

/// The argv `run --auto` executes for a command the hook may wrap, or None to
/// leave it to the host. The command must parse in the shell subset of
/// `shell`, every command in it must be a recognized noninteractive one (or
/// `cd DIR &&`, `|| true`, or a read-only filter after a pipe), and its
/// output must not already be bounded by a final `head`, `tail`, `wc` or
/// `grep -c`/`-l`/`-q`.
fn command_args(command: &str) -> Option<(Vec<String>, Profile)> {
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
        let profile = crate::commands::profile(&first.argv)?;
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
fn claude_fields(event: &Value) -> Option<ClaudeResult<'_>> {
    let output = &event["tool_response"];
    if event["tool_name"] == "Grep" {
        if output["mode"] != "content" || !output["content"].is_string() {
            return None;
        }
        let anchors = event["tool_input"]["pattern"]
            .as_str()
            .map(crate::commands::pattern_anchors)
            .unwrap_or_default();
        return Some(ClaudeResult {
            fields: &["content"],
            profile: Some(Profile::Search),
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
    let (profile, small) = limits(command);
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
fn claude_result(event: &Value) -> Result<Value> {
    let Some(result) = claude_fields(event) else {
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
    let program = crate::commands::program(command);
    let program = program.as_deref();
    let unchanged = |reason| crate::events::unchanged("claude", program, profile, total, reason);
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
    // A field identical to one this context was already shown (whole, as a
    // view or raw) is replaced by a reference to it.
    let mut repeated = vec![false; fields.len()];
    let context = context_key(event);
    // What the context had been shown when this result arrived. The outputs
    // this result shows are added in one locked step at the end, since
    // other hooks of the same context run meanwhile.
    let state = context.as_deref().map(crate::session::load);
    let mut shown: Vec<String> = Vec::new();
    if let Some(before) = &state {
        for (i, field) in fields.iter().enumerate() {
            let raw = output[*field].as_str().unwrap();
            if raw.len() <= compress::SMALL || raw.len() > MAX_INPUT {
                continue;
            }
            let hash = digest(raw.as_bytes());
            let hash = crate::session::short(&hash);
            // Seen before this result: its other stream holding the same
            // bytes is as new to the conversation as this one.
            if before.saw(hash)
                && let Some(marker) =
                    compress::repeat_recorded("claude", program, raw.as_bytes(), None, profile)
            {
                output[*field] = json!(String::from_utf8(marker)?);
                repeated[i] = true;
                changed = true;
            }
            shown.push(hash.to_owned());
        }
    }
    let remembered = || {
        // Nothing to write for most results: they are small and remember
        // nothing, and a hook runs on every tool call.
        if let Some(key) = context.as_deref().filter(|_| !shown.is_empty()) {
            // Best effort: a lost write only misses a later repeat.
            let _ = crate::session::update(key, false, |state| {
                for hash in &shown {
                    state.remember(hash);
                }
            });
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
    // Only a result that may be compressed needs its anchors.
    let mut anchors = anchors(event, state.as_ref(), command);
    anchors.extend(extra);
    for (i, field) in fields.iter().enumerate() {
        if repeated[i] {
            continue;
        }
        let raw = output[*field].as_str().unwrap();
        if raw.len() > MAX_INPUT {
            return Ok(json!({}));
        }
        let small = compress::automatic_journaled(
            "claude",
            program,
            raw.as_bytes(),
            None,
            profile,
            None,
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
    terms.retain(|t| !crate::session::incidental(t));
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
    let start = name == "SessionStart";
    // A start rewrites the file so cleanup sees the context as recent.
    let announce = crate::session::update(&key, start, |state| {
        if start && matches!(event["source"].as_str(), Some("compact" | "clear")) {
            state.seen.clear();
        }
        if start && event["source"] == "clear" {
            state.anchors.clear();
        }
        // A prompt, or a context started over, opens a new turn.
        state.new_turn();
        // Prompt terms stay in this private state, never in the event journal.
        if name == "UserPromptSubmit"
            && let Some(prompt) = event["prompt"].as_str()
        {
            state.anchor(crate::session::prompt_anchors(prompt));
        }
        let announce = start || state.mode != Some(mode);
        state.mode = Some(mode);
        announce
    })?;
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
/// output stays exact: the profile's, the default otherwise.
fn limits(command: &str) -> (Option<Profile>, usize) {
    let profile = crate::commands::classify_lenient(command);
    (profile, profile.map_or(compress::SMALL, Profile::small))
}

/// OpenCode's plugin sends two events: a system-prompt pass (rebuilt on every
/// model step, so no per-session state) and one bash result to compress.
fn opencode(event: &Value, mode: Preference) -> Result<Value> {
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
    let (profile, small) = limits(command);
    let program = crate::commands::program(command);
    if raw.len() <= small {
        crate::events::unchanged(
            "opencode",
            program.as_deref(),
            profile,
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
    let small = compress::automatic_journaled(
        "opencode",
        program.as_deref(),
        raw.as_bytes(),
        None,
        profile,
        None,
        &anchors,
    );
    if small.as_ref() == raw.as_bytes() {
        return Ok(json!({}));
    }
    Ok(json!({"output": String::from_utf8(small.into_owned())?}))
}
pub fn hook_stdin(agent: Agent) -> Result<Value> {
    ensure!(agent != Agent::All, "hook needs one agent");
    let mut bytes = Vec::new();
    std::io::stdin()
        .take((MAX_INPUT * 3 + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_INPUT * 3, "hook input too large");
    let event: Value = serde_json::from_slice(&bytes)?;
    // Lets the tests check that a panic still leaves the host's output alone.
    #[cfg(debug_assertions)]
    if std::env::var_os("SCOPELET_TEST_HOOK_PANIC").is_some() {
        panic!("test panic");
    }
    hook(agent, &event)
}
fn hook(agent: Agent, event: &Value) -> Result<Value> {
    let mode = preference()?;
    if agent == Agent::Opencode {
        return opencode(event, mode);
    }
    let name = event["hook_event_name"].as_str().unwrap_or("");
    if matches!(name, "SessionStart" | "UserPromptSubmit") {
        let mut out = context(event, mode)?;
        // Claude Code shows `systemMessage` to the person, not the model.
        if agent == Agent::Claude
            && name == "SessionStart"
            && mode != Preference::Off
            && let Some(version) = newer_on_path()
        {
            out["systemMessage"] = json!(format!(
                "Scopelet: hooks run the pinned scopelet {}, older than {version} on the PATH; run `scopelet install --agent all` and restart sessions.",
                env!("CARGO_PKG_VERSION")
            ));
        }
        return Ok(out);
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
        let Some((args, profile)) = command_args(command) else {
            return Ok(json!({}));
        };
        if small_file_read(&args, profile.small(), event) {
            return Ok(json!({}));
        }
        let binary = std::env::current_exe()?;
        // The context key lets the wrapped run read this context's anchors.
        let session = context_key(event).map_or(String::new(), |key| {
            format!("--session {} ", crate::session::short(&key))
        });
        let command = format!(
            "{} run --auto --timeout 3600 {session}--profile {} -- {}",
            quote(&binary.to_string_lossy()),
            profile.name(),
            args.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" ")
        );
        let mut input = event["tool_input"].clone();
        input["command"] = json!(command);
        return Ok(
            json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":input}}),
        );
    }
    if agent == Agent::Claude && name == "PostToolUse" {
        let mut out = claude_result(event)?;
        loop_note(event, name, &mut out);
        return Ok(out);
    }
    if agent == Agent::Claude && name == "PostToolUseFailure" {
        let mut out = json!({});
        loop_note(event, name, &mut out);
        return Ok(out);
    }
    Ok(json!({}))
}

/// Occurrences of one failure within a turn that earn a loop note.
pub const LOOP_REPEATS: u8 = 3;

/// Whether loop notes are given: `SCOPELET_LOOPGUARD=0` turns them off.
fn loop_guard() -> bool {
    std::env::var_os("SCOPELET_LOOPGUARD").is_none_or(|v| v != "0")
}

/// Claude Code: when a Bash command reports the same failure (by
/// `compress::failure_signature`) for the `LOOP_REPEATS`th time in one turn,
/// add a note asking for another hypothesis, at most once per turn. A failed
/// call (`PostToolUseFailure`, whose `error` carries the output) always
/// counts; a call that exited 0 counts only for a build, test or check (its
/// output piped through `tail`, say). The note mirrors what the agent did and
/// never stops anything; the output itself is left as `out` already has it.
fn loop_note(event: &Value, name: &str, out: &mut Value) {
    if !loop_guard() || event["tool_name"] != "Bash" {
        return;
    }
    let command = event["tool_input"]["command"].as_str().unwrap_or("");
    if crate::commands::invokes_wrapper(command) {
        return;
    }
    let profile = crate::commands::classify_lenient(command);
    let failed = name == "PostToolUseFailure";
    if !failed && profile != Some(Profile::Tests) {
        return;
    }
    let output = &event["tool_response"];
    let text = if failed {
        event["error"].as_str().unwrap_or("").to_owned()
    } else {
        let stream = |field: &str| output[field].as_str().unwrap_or("");
        format!("{}\n{}", stream("stdout"), stream("stderr"))
    };
    if text.len() > MAX_INPUT {
        return;
    }
    let Some((signature, label)) = compress::failure_signature(&text) else {
        return;
    };
    let Some(key) = context_key(event) else {
        return;
    };
    let program = crate::commands::program(command);
    let shown = program.as_deref().unwrap_or("command");
    let signature = digest(format!("{shown}\n{signature}").as_bytes());
    // Counted in one locked step: two failures reported side by side are
    // two, not one. Best effort: a lost write only delays a note.
    let (count, note) = crate::session::update(&key, false, |state| {
        let count = state.failed(crate::session::short(&signature));
        let note = count >= LOOP_REPEATS && !state.nudged;
        state.nudged |= note;
        (count, note)
    })
    .unwrap_or((0, false));
    if !note {
        return;
    }
    crate::events::looped("claude", program.as_deref(), profile);
    let text = format!(
        "Scopelet: the same failure ({shown}: {label}) appeared {count} times this turn. Re-running will not change it: re-read the failing expectation and test a different hypothesis before editing again."
    );
    match out.get_mut("hookSpecificOutput") {
        Some(specific) => specific["additionalContext"] = json!(text),
        None => {
            *out = json!({"hookSpecificOutput":{"hookEventName":name,"additionalContext":text}})
        }
    }
}
