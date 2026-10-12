//! Private per-context hook state, one file per conversation context under
//! `<config>/sessions/`: the response mode last announced, the outputs already
//! shown in that context and the relevance anchors of its prompts. A
//! subagent is a context of its own. Nothing here reaches the event journal.
use crate::{integration::Preference, store::digest};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

/// Outputs remembered per context; the oldest are forgotten first.
pub const MAX_SEEN: usize = 512;
/// Hex characters kept of an output's hash, and of a context key passed on a
/// command line (`run --session`).
pub const PREFIX: usize = 16;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct State {
    /// The mode whose guidance this context last received.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Preference>,
    /// Hash prefixes of outputs already shown, oldest first.
    #[serde(default)]
    pub seen: Vec<String>,
    /// Terms of recent prompts, oldest first.
    #[serde(default)]
    pub anchors: Vec<String>,
}

impl State {
    /// Whether output with this hash prefix was already shown here.
    pub fn saw(&self, hash: &str) -> bool {
        self.seen.iter().any(|h| h == hash)
    }
    /// Remember output with this hash prefix as shown, forgetting the oldest
    /// beyond `MAX_SEEN`.
    pub fn remember(&mut self, hash: &str) {
        self.seen.retain(|h| h != hash);
        self.seen.push(hash.to_owned());
        let excess = self.seen.len().saturating_sub(MAX_SEEN);
        self.seen.drain(..excess);
    }
}

/// The file name of a context: the session, and the subagent inside it when
/// there is one. The main context keeps the name earlier versions used.
pub fn key(session: &str, agent: Option<&str>) -> String {
    match agent.filter(|a| !a.is_empty()) {
        Some(agent) => digest(format!("{session}\n{agent}").as_bytes()),
        None => digest(session.as_bytes()),
    }
}

/// The hash prefix by which a context or an output is remembered.
pub fn short(hash: &str) -> &str {
    &hash[..PREFIX.min(hash.len())]
}

fn folder() -> Result<PathBuf> {
    Ok(crate::integration::root()?.join("sessions"))
}

/// The state of a context; a missing or unreadable file is a fresh context.
/// Earlier versions stored the bare mode, which is read as such.
pub fn load(key: &str) -> State {
    let Ok(bytes) = folder().and_then(|f| Ok(fs::read(f.join(key))?)) else {
        return State::default();
    };
    serde_json::from_slice(&bytes)
        .or_else(|_| {
            serde_json::from_slice::<Preference>(&bytes).map(|mode| State {
                mode: Some(mode),
                ..State::default()
            })
        })
        .unwrap_or_default()
}

pub fn save(key: &str, state: &State) -> Result<()> {
    crate::integration::write_atomic(&folder()?.join(key), &serde_json::to_vec(state)?)
}

/// The full key of the one context whose key starts with `prefix` (at least
/// `PREFIX` hex characters), or None.
pub fn find(prefix: &str) -> Option<String> {
    if prefix.len() < PREFIX || !prefix.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let prefix = prefix.to_ascii_lowercase();
    let mut found = fs::read_dir(folder().ok()?)
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.len() == 64 && name.starts_with(&prefix));
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seen_outputs_are_bounded_and_refreshed() {
        let mut state = State::default();
        for i in 0..MAX_SEEN + 10 {
            state.remember(&format!("{i:016x}"));
        }
        assert_eq!(state.seen.len(), MAX_SEEN);
        assert!(!state.saw(&format!("{:016x}", 0)));
        assert!(state.saw(&format!("{:016x}", MAX_SEEN + 9)));
        // Seeing an output again makes it the most recent.
        state.remember(&format!("{:016x}", 10));
        assert_eq!(state.seen.last().unwrap(), &format!("{:016x}", 10));
        assert_eq!(state.seen.len(), MAX_SEEN);
    }

    #[test]
    fn a_subagent_has_its_own_context() {
        assert_eq!(key("s", None), digest(b"s"));
        assert_eq!(key("s", Some("")), digest(b"s"));
        assert_ne!(key("s", Some("a1")), key("s", None));
        assert_ne!(key("s", Some("a1")), key("s", Some("a2")));
    }
}
