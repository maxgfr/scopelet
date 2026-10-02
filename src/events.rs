//! A local journal of compression decisions, never transmitted.
//!
//! One JSON line per event in `<cache>/events/YYYY-MM.jsonl`, under 512
//! bytes: time, host, kind, profile, byte counts, reason, compact version and
//! 12-character prefixes of the artifact and original it produced. No
//! content, command or path is recorded. Writing is best effort and
//! `SCOPELET_EVENTS=0` turns it off.
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
    /// `compress` or `expand`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub bytes_in: usize,
    pub bytes_out: usize,
    /// Why the output was or was not replaced (`compress::Reason`).
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
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

fn version_number(version: crate::compress::Version) -> u8 {
    match version {
        crate::compress::Version::V1 => 1,
        crate::compress::Version::V2 => 2,
        crate::compress::Version::V3 => 3,
    }
}

/// Journal an output left unchanged before compression was attempted.
pub fn unchanged(
    host: &str,
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
            kind: "compress".into(),
            profile: profile.map(|p| p.name().into()),
            bytes_in: bytes,
            bytes_out: bytes,
            reason: reason.as_str().into(),
            version: Some(version_number(version)),
            ..Event::default()
        },
    );
}

/// Compress like `compress::automatic_profile` and journal the decision. Any
/// failure leaves the original bytes, journaled as `error`.
pub fn automatic<'a>(
    host: &str,
    bytes: &'a [u8],
    cache: Option<PathBuf>,
    profile: Option<crate::commands::Profile>,
    budget: Option<usize>,
    version: crate::compress::Version,
) -> std::borrow::Cow<'a, [u8]> {
    let decided =
        crate::compress::automatic_decision(bytes, cache.clone(), profile, budget, version);
    let (output, reason, artifact, blob) = match decided {
        Ok((output, decision)) => (
            output,
            decision.reason.as_str(),
            decision.artifact,
            decision.blob,
        ),
        Err(_) => (std::borrow::Cow::Borrowed(bytes), "error", None, None),
    };
    if !bytes.is_empty() {
        record(
            cache,
            Event {
                host: host.into(),
                kind: "compress".into(),
                profile: profile
                    .filter(|_| version == crate::compress::Version::V3)
                    .map(|p| p.name().into()),
                bytes_in: bytes.len(),
                bytes_out: output.len(),
                reason: reason.into(),
                version: Some(version_number(version)),
                artifact: artifact.map(|a| prefix(&a)),
                blob: blob.map(|b| prefix(&b)),
                ..Event::default()
            },
        );
    }
    output
}

/// Journal a recovery of a stored reference.
pub fn expanded(cache: Option<PathBuf>, reference: &str) {
    record(
        cache,
        Event {
            host: "cli".into(),
            kind: "expand".into(),
            reason: "expand".into(),
            artifact: Some(prefix(reference)),
            ..Event::default()
        },
    );
}

/// What `scopelet stats` reports.
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Stats {
    pub days: u64,
    pub events: usize,
    pub compressions: usize,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub bytes_saved: u64,
    /// Outputs left unchanged, by reason.
    pub pass_through: BTreeMap<String, usize>,
    /// Compressions by profile (`none` when unrecognized).
    pub by_profile: BTreeMap<String, usize>,
    pub expands: usize,
    /// Compressions whose artifact or original was expanded afterwards.
    pub expanded_after_compression: usize,
    /// `expanded_after_compression / compressions`, when there were any.
    pub expand_rate: Option<f64>,
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
        events: events.len(),
        journal,
        ..Stats::default()
    };
    let mut compressed: Vec<&Event> = Vec::new();
    for event in &events {
        match event.kind.as_str() {
            "compress" if event.reason == "compressed" => {
                stats.compressions += 1;
                stats.bytes_in += event.bytes_in as u64;
                stats.bytes_out += event.bytes_out as u64;
                *stats
                    .by_profile
                    .entry(event.profile.clone().unwrap_or_else(|| "none".into()))
                    .or_default() += 1;
                compressed.push(event);
            }
            "compress" => *stats.pass_through.entry(event.reason.clone()).or_default() += 1,
            "expand" => stats.expands += 1,
            _ => {}
        }
    }
    stats.bytes_saved = stats.bytes_in.saturating_sub(stats.bytes_out);
    // An expansion counts for a compression it follows and names.
    stats.expanded_after_compression = compressed
        .iter()
        .filter(|c| {
            events.iter().any(|e| {
                e.kind == "expand"
                    && e.ts >= c.ts
                    && e.artifact.as_ref().is_some_and(|x| {
                        Some(x) == c.artifact.as_ref() || Some(x) == c.blob.as_ref()
                    })
            })
        })
        .count();
    stats.expand_rate = (stats.compressions > 0)
        .then(|| stats.expanded_after_compression as f64 / stats.compressions as f64);
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
