//! Private per-context hook state, one file per conversation context under
//! `<config>/sessions/`: the response mode last announced, the outputs already
//! shown in that context and the relevance anchors of its prompts. A
//! subagent is a context of its own. Nothing here reaches the event journal.
use crate::{integration::Preference, store::digest};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, sync::LazyLock};

/// Outputs remembered per context; the oldest are forgotten first.
pub const MAX_SEEN: usize = 512;
/// Hex characters kept of an output's hash, and of a context key passed on a
/// command line (`run --session`).
pub const PREFIX: usize = 16;
/// Prompt terms remembered per context; the newest replace the oldest.
pub const MAX_ANCHORS: usize = 32;
/// Byte length of a term worth matching: shorter ones match everywhere.
pub const ANCHOR_BYTES: std::ops::RangeInclusive<usize> = 3..=64;

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
    /// Add a prompt's terms, newest last, forgetting the oldest beyond
    /// `MAX_ANCHORS`.
    pub fn anchor(&mut self, terms: Vec<String>) {
        for term in terms {
            self.anchors.retain(|t| *t != term);
            self.anchors.push(term);
        }
        let excess = self.anchors.len().saturating_sub(MAX_ANCHORS);
        self.anchors.drain(..excess);
    }
}

/// Quoted text, paths, identifiers, error codes and long numbers.
static TERM: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(concat!(
        r#""([^"\n]+)"|`([^`\n]+)`"#,
        r#"|(?:^|[\s(\[{:,])'([^'\s](?:[^'\n]*[^'\s])?)'(?:$|[\s)\]}.,:;!?])"#,
        r"|((?:\.{0,2}/)?[\w.-]+(?:/[\w.-]+)+/?|[\w-]{2,}\.[A-Za-z][A-Za-z0-9]{0,4}\b)",
        r"|([A-Za-z_][A-Za-z0-9_]*)",
        r"|(\b[0-9]{3,}\b)",
    ))
    .unwrap()
});

/// Whether a bare word names something specific: snake_case, CamelCase or
/// mixedCase, or an error code (`E0502`, `TS2345`). A capitalized or upper
/// case word alone is ordinary prose.
fn specific(word: &str) -> bool {
    let has = |f: fn(&char) -> bool| word.chars().any(|c| f(&c));
    let upper = word.chars().filter(char::is_ascii_uppercase).count();
    let first_lower = word.starts_with(|c: char| c.is_ascii_lowercase());
    let code = word.starts_with(|c: char| c.is_ascii_uppercase())
        && word.ends_with(|c: char| c.is_ascii_digit())
        && word
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    (word.contains('_') && has(char::is_ascii_alphanumeric))
        || (has(char::is_ascii_lowercase) && (upper >= 2 || (first_lower && upper >= 1)))
        || code
}

/// The terms of a prompt an output line is likely to contain, in order of
/// first appearance: quoted text, paths and file names, identifiers in
/// snake_case or CamelCase, error codes and numbers of three digits or more.
/// Deterministic and literal; 3 to 64 bytes each, at most `MAX_ANCHORS`.
pub fn prompt_anchors(prompt: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for found in TERM.captures_iter(prompt) {
        let term = if let Some(m) = found.get(4) {
            m.as_str().trim_end_matches(['.', ',', '/'])
        } else if let Some(m) = found.get(5) {
            if !specific(m.as_str()) {
                continue;
            }
            m.as_str()
        } else {
            match (1..=6).find_map(|i| found.get(i)) {
                Some(m) => m.as_str().trim(),
                None => continue,
            }
        };
        if ANCHOR_BYTES.contains(&term.len()) && !terms.iter().any(|t| t == term) {
            terms.push(term.to_owned());
        }
        if terms.len() == MAX_ANCHORS {
            break;
        }
    }
    terms
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
    fn prompt_anchors_keep_specific_terms_only() {
        let prompt = "Why does `parse_header` fail on src/net/http.rs? The \
            HttpClient says \"connection reset by peer\" with E0502 near line 4127, \
            see l'agent's 'retry-policy' and config.yaml. Fix It Now.";
        assert_eq!(
            prompt_anchors(prompt),
            [
                "parse_header",
                "src/net/http.rs",
                "HttpClient",
                "connection reset by peer",
                "E0502",
                "4127",
                "retry-policy",
                "config.yaml",
            ]
        );
        assert!(prompt_anchors("ok, do it now please").is_empty());
        let many: String = (0..100).map(|i| format!("term_{i} ")).collect();
        assert_eq!(prompt_anchors(&many).len(), MAX_ANCHORS);
        let long = format!("\"{}\"", "x".repeat(65));
        assert!(prompt_anchors(&long).is_empty());
    }

    #[test]
    fn newer_anchors_replace_the_oldest() {
        let mut state = State::default();
        state.anchor((0..31).map(|i| format!("old_{i}")).collect());
        state.anchor(vec!["old_0".into(), "new_a".into(), "new_b".into()]);
        assert_eq!(state.anchors.len(), MAX_ANCHORS);
        assert_eq!(state.anchors[..2], ["old_2", "old_3"]);
        assert_eq!(
            state.anchors[MAX_ANCHORS - 3..],
            ["old_0", "new_a", "new_b"]
        );
    }

    #[test]
    fn a_subagent_has_its_own_context() {
        assert_eq!(key("s", None), digest(b"s"));
        assert_eq!(key("s", Some("")), digest(b"s"));
        assert_ne!(key("s", Some("a1")), key("s", None));
        assert_ne!(key("s", Some("a1")), key("s", Some("a2")));
    }
}
