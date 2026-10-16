//! A local journal of compression decisions, never transmitted.
//!
//! One JSON line per event in `<cache>/events/YYYY-MM.jsonl`, under 512
//! bytes: time, host, kind, profile, byte counts, reason, compact version,
//! 12-character prefixes of the artifact and original it produced, whether
//! relevance anchors ranked lines up, the bare name of the program that
//! printed the output, and for an expansion its kind. No content, argument,
//! path or anchor term is recorded. Writing is best effort and
//! `SCOPELET_EVENTS=0` turns it off.
use crate::compress::Reason;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Hash characters kept to correlate a compression with a later `expand`.
const PREFIX: usize = 12;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Event {
    /// UTC, RFC 3339 to the second.
    pub ts: String,
    /// `claude`, `opencode`, `run` (Codex's wrapper and direct use), `cli`.
    pub host: String,
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// The bare name of the program that printed the output (its first
    /// recognized command's, else its first), `other` when it is not a
    /// short plain name; none when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    pub bytes_in: usize,
    pub bytes_out: usize,
    /// Why the output was or was not replaced; none for an expansion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
    /// A compression in which relevance anchors ranked lines up.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub anchored: bool,
    /// How an expansion read the evidence: `find`, `range`, `page`,
    /// `manifest` or `raw`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expand: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// An automatic compression decision.
    #[default]
    Compress,
    /// A recovery with `expand`.
    Expand,
}

/// The first characters of a reference's hash.
pub fn prefix(reference: &str) -> String {
    let hash = reference.split_once(':').map_or(reference, |(_, h)| h);
    hash.chars().take(PREFIX).collect()
}

pub fn enabled() -> bool {
    std::env::var_os("SCOPELET_EVENTS").is_none_or(|v| v != "0")
}

/// Days since 1970-01-01 to a civil date (proleptic Gregorian).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// RFC 3339 UTC time to the second.
pub fn timestamp(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let (year, month, day) = civil(secs.div_euclid(86_400));
    let rest = secs.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

fn folder(cache: Option<PathBuf>) -> PathBuf {
    crate::store::cache_root(cache).join("events")
}

/// Append one event; any failure is ignored.
pub fn record(cache: Option<PathBuf>, mut event: Event) {
    if !enabled() {
        return;
    }
    let _ = (|| -> std::io::Result<()> {
        let now = SystemTime::now();
        event.ts = timestamp(now);
        let mut line = serde_json::to_vec(&event)?;
        if line.len() >= 512 {
            return Ok(());
        }
        line.push(b'\n');
        let folder = folder(cache);
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&folder)?;
        let _ = crate::store::ignore_cached_evidence(&folder);
        // One write of a short line to a file opened for appending: lines
        // from concurrent hooks do not interleave.
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(folder.join(format!("{}.jsonl", &event.ts[..7])))?
            .write_all(&line)
    })();
}

/// Journal an output left unchanged before compression was attempted.
pub fn unchanged(
    host: &str,
    program: Option<&str>,
    profile: Option<crate::commands::Profile>,
    version: crate::compress::Version,
    bytes: usize,
    reason: crate::compress::Reason,
) {
    if bytes == 0 {
        return;
    }
    record(
        None,
        Event {
            host: host.into(),
            kind: Kind::Compress,
            profile: crate::compress::effective_profile(profile, version).map(|p| p.name().into()),
            program: program.map(str::to_owned),
            bytes_in: bytes,
            bytes_out: bytes,
            reason: Some(reason),
            version: Some(version.number()),
            ..Event::default()
        },
    );
}

/// Journal a recovery of a stored reference and how it read it.
pub fn expanded(cache: Option<PathBuf>, reference: &str, how: &str) {
    record(
        cache,
        Event {
            host: "cli".into(),
            kind: Kind::Expand,
            artifact: Some(prefix(reference)),
            expand: Some(how.into()),
            ..Event::default()
        },
    );
}

/// Programs `stats` lists among compressions without a profile.
const UNPROFILED_LISTED: usize = 10;

/// Expansions above this share of a profile's compressions suggest its
/// budget is too tight (with at least `TIGHT_MIN` compressions).
const TIGHT_RATE: f64 = 0.25;
/// Compressions needed before a profile's expand rate means anything.
const TIGHT_MIN: usize = 4;

/// One profile's figures in `scopelet stats`.
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct ProfileStats {
    pub compressions: usize,
    /// Compressions whose artifact or original was expanded afterwards.
    pub expanded: usize,
    pub expand_rate: Option<f64>,
    /// Compressions in which relevance anchors ranked lines up.
    pub anchored: usize,
    /// Outputs replaced by a reference to an identical one already shown.
    pub repeats: usize,
    pub repeat_bytes_saved: u64,
    /// More than 25% of at least four compressions were expanded: the
    /// views probably leave out what the agent needs. A report, not a
    /// setting: nothing is adjusted automatically.
    pub budget_probably_tight: bool,
}

/// What `scopelet stats` reports.
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Stats {
    pub days: u64,
    /// False when `SCOPELET_EVENTS=0`: the counts below then cover only what
    /// was journaled before, not what happened since.
    pub journal_enabled: bool,
    pub events: usize,
    pub compressions: usize,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub bytes_saved: u64,
    /// Outputs left unchanged, by reason.
    pub pass_through: BTreeMap<String, usize>,
    /// Compressions by profile (`none` when unrecognized).
    pub by_profile: BTreeMap<String, usize>,
    /// Compressions by program (`unknown` before this was journaled).
    pub by_program: BTreeMap<String, usize>,
    /// The programs whose compressions had no profile, most frequent first
    /// (at most `UNPROFILED_LISTED`): where a new profile would help most.
    pub unprofiled_programs: Vec<(String, usize)>,
    pub expands: usize,
    /// Expansions by how they read the evidence (`find`, `range`, `page`,
    /// `manifest`, `raw`; `unknown` before this was journaled).
    pub expands_by_kind: BTreeMap<String, usize>,
    /// Compressions whose artifact or original was expanded afterwards.
    pub expanded_after_compression: usize,
    /// `expanded_after_compression / compressions`, when there were any.
    pub expand_rate: Option<f64>,
    /// Outputs replaced by a reference to one already shown, and the bytes
    /// that saved.
    pub repeats: usize,
    pub repeat_bytes_saved: u64,
    /// Compressions in which relevance anchors ranked lines up.
    pub anchored: usize,
    /// The same figures per profile (`none` when unrecognized).
    pub profiles: BTreeMap<String, ProfileStats>,
    pub journal: PathBuf,
}

/// Summarize the journal over the last `days` days.
pub fn stats(cache: Option<PathBuf>, days: u64) -> Stats {
    let journal = folder(cache);
    let cutoff = timestamp(SystemTime::now() - Duration::from_secs(days.saturating_mul(86_400)));
    let mut events: Vec<Event> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&journal) {
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
            // A month file can only hold recent events if its month is not
            // before the cutoff's.
            .filter(|p| month_of(p).is_some_and(|m| *m >= cutoff[..7]))
            .collect();
        files.sort();
        for file in files {
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            events.extend(
                text.lines()
                    .filter_map(|line| serde_json::from_str::<Event>(line).ok())
                    .filter(|e| e.ts >= cutoff),
            );
        }
    }
    let mut stats = Stats {
        days,
        journal_enabled: enabled(),
        events: events.len(),
        journal,
        ..Stats::default()
    };
    let mut compressed: Vec<&Event> = Vec::new();
    let profile_of = |e: &Event| e.profile.clone().unwrap_or_else(|| "none".into());
    let program_of = |e: &Event| e.program.clone().unwrap_or_else(|| "unknown".into());
    let mut unprofiled: BTreeMap<String, usize> = BTreeMap::new();
    for event in &events {
        match (event.kind, event.reason) {
            (Kind::Compress, Some(Reason::Compressed)) => {
                stats.compressions += 1;
                stats.bytes_in += event.bytes_in as u64;
                stats.bytes_out += event.bytes_out as u64;
                *stats.by_profile.entry(profile_of(event)).or_default() += 1;
                *stats.by_program.entry(program_of(event)).or_default() += 1;
                if event.profile.is_none() {
                    *unprofiled.entry(program_of(event)).or_default() += 1;
                }
                let profile = stats.profiles.entry(profile_of(event)).or_default();
                profile.compressions += 1;
                if event.anchored {
                    stats.anchored += 1;
                    profile.anchored += 1;
                }
                compressed.push(event);
            }
            (Kind::Compress, Some(Reason::Repeat)) => {
                let saved = event.bytes_in.saturating_sub(event.bytes_out) as u64;
                stats.repeats += 1;
                stats.repeat_bytes_saved += saved;
                let profile = stats.profiles.entry(profile_of(event)).or_default();
                profile.repeats += 1;
                profile.repeat_bytes_saved += saved;
            }
            (Kind::Compress, Some(reason)) => {
                *stats
                    .pass_through
                    .entry(reason.as_str().into())
                    .or_default() += 1
            }
            (Kind::Expand, _) => {
                stats.expands += 1;
                *stats
                    .expands_by_kind
                    .entry(event.expand.clone().unwrap_or_else(|| "unknown".into()))
                    .or_default() += 1;
            }
            (Kind::Compress, None) => {}
        }
    }
    stats.bytes_saved = stats.bytes_in.saturating_sub(stats.bytes_out);
    let mut unprofiled: Vec<(String, usize)> = unprofiled.into_iter().collect();
    // Most frequent first; equal counts keep name order (a stable sort).
    unprofiled.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    unprofiled.truncate(UNPROFILED_LISTED);
    stats.unprofiled_programs = unprofiled;
    // An expansion counts for a compression it follows and names.
    let expanded = |c: &&&Event| {
        events.iter().any(|e| {
            e.kind == Kind::Expand
                && e.ts >= c.ts
                && e.artifact
                    .as_ref()
                    .is_some_and(|x| Some(x) == c.artifact.as_ref() || Some(x) == c.blob.as_ref())
        })
    };
    for event in compressed.iter().filter(expanded) {
        stats.expanded_after_compression += 1;
        stats
            .profiles
            .entry(profile_of(event))
            .or_default()
            .expanded += 1;
    }
    stats.expand_rate = (stats.compressions > 0)
        .then(|| stats.expanded_after_compression as f64 / stats.compressions as f64);
    for profile in stats.profiles.values_mut() {
        profile.expand_rate = (profile.compressions > 0)
            .then(|| profile.expanded as f64 / profile.compressions as f64);
        profile.budget_probably_tight = profile.compressions >= TIGHT_MIN
            && profile.expand_rate.is_some_and(|rate| rate > TIGHT_RATE);
    }
    stats
}

fn month_of(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    (stem.len() == 7 && stem.as_bytes()[4] == b'-').then(|| stem.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_utc_civil_dates() {
        let at = |secs: u64| timestamp(UNIX_EPOCH + Duration::from_secs(secs));
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(at(1_790_000_000), "2026-09-21T14:13:20Z");
        assert_eq!(at(4_102_444_799), "2099-12-31T23:59:59Z");
    }

    #[test]
    fn prefixes_drop_the_reference_type() {
        assert_eq!(
            prefix(&format!("artifact:{}", "ab".repeat(32))),
            "abababababab"
        );
        assert_eq!(prefix("0123456789abcdef"), "0123456789ab");
    }
}
