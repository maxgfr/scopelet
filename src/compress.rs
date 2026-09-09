//! Recoverable presentation. Selection borrows evidence; storage follows acceptance.
use crate::{compact_table, encoding, model::*, sources::Parsed, store::Store};
use anyhow::{Result, ensure};
use std::{borrow::Cow, collections::BTreeSet, path::PathBuf, sync::LazyLock};

pub const DEFAULT_BUDGET: usize = 4096;
pub const SMALL: usize = 2048;
const PLACEHOLDER: &str =
    "artifact:0000000000000000000000000000000000000000000000000000000000000000";
static SIGNAL: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(error|failed|failure|panic|panicked|exception|traceback|warning|assertionerror|caused by|test result|tests? passed|tests? failed)\b|^\s*(FAIL|PASS|E\s+|FATAL|×|✕)").unwrap()
});

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Version {
    #[value(name = "1")]
    V1,
    #[default]
    #[value(name = "2")]
    V2,
}
impl Version {
    pub fn configured(explicit: Option<Self>) -> Result<Self> {
        if let Some(version) = explicit {
            return Ok(version);
        }
        match std::env::var("SCOPELET_COMPACT_VERSION") {
            Ok(v) if v == "1" => Ok(Self::V1),
            Ok(v) if v == "2" => Ok(Self::V2),
            Err(std::env::VarError::NotPresent) => Ok(Self::default()),
            _ => anyhow::bail!("SCOPELET_COMPACT_VERSION must be 1 or 2"),
        }
    }
}

/// Conservative detection retained for callers; automatic ingestion reuses parsed values.
pub fn detect(text: &str) -> Format {
    match Parsed::detected(text.to_owned()) {
        Parsed::Text(_) => Format::Text,
        Parsed::Json(_) => Format::Json,
        Parsed::Jsonl(_) => Format::Jsonl,
    }
}

pub fn automatic(bytes: &[u8], store: &Store, budget: usize) -> Result<Vec<u8>> {
    automatic_with(bytes, budget, Version::V1, || Ok(store.clone())).map(Cow::into_owned)
}

/// The store factory is not invoked for a rejected or ineligible substitution.
pub fn automatic_lazy<'a>(
    bytes: &'a [u8],
    path: Option<PathBuf>,
    budget: usize,
    version: Version,
) -> Result<Cow<'a, [u8]>> {
    automatic_with(bytes, budget, version, || Store::open(path))
}

fn automatic_with(
    bytes: &[u8],
    budget: usize,
    version: Version,
    store: impl FnOnce() -> Result<Store>,
) -> Result<Cow<'_, [u8]>> {
    ensure!(
        (1024..=1024 * 1024).contains(&budget),
        "max_bytes must be 1024..1048576"
    );
    if bytes.len() <= SMALL || bytes.iter().filter(|&&b| b == b'\n').count() > 100_000 {
        return Ok(Cow::Borrowed(bytes));
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Ok(Cow::Borrowed(bytes));
    };
    if text.contains("[scopelet ") || text.contains("<persisted-output>") {
        return Ok(Cow::Borrowed(bytes));
    }
    let mut data = Dataset {
        records: Parsed::detected(text.to_owned()).records("input", String::new()),
        ..Dataset::default()
    };
    data.notes
        .push("Bytes supplied to the compressor; completeness before capture is unknown.".into());
    let (mut result, shown) = view(&data, PLACEHOLDER, budget, version)?;
    if shown == 0 || result.len() + 512 > bytes.len() || result.len() * 5 > bytes.len() * 4 {
        return Ok(Cow::Borrowed(bytes));
    }
    let store = store()?;
    let blob = store.put("blob", bytes)?;
    for record in &mut data.records {
        record.blob = Some(blob.clone());
    }
    data.snapshots.push(Snapshot {
        source: "input".into(),
        blob,
        bytes: bytes.len(),
        local_path: None,
    });
    let artifact = store.put_json(&data)?;
    // Only substitute our header, never placeholder-looking text in original evidence.
    let end = result.find('\n').unwrap() + 1;
    result.replace_range(..end, &header(&data, &artifact, version));
    ensure!(result.len() <= budget, "compact metadata exceeds budget");
    Ok(Cow::Owned(result.into_bytes()))
}

pub fn compact(data: &Dataset, store: &Store, budget: usize) -> Result<String> {
    compact_version(data, store, budget, Version::V1)
}
pub fn compact_version(
    data: &Dataset,
    store: &Store,
    budget: usize,
    version: Version,
) -> Result<String> {
    let (mut text, _) = view(data, PLACEHOLDER, budget, version)?;
    let id = store.put_json(data)?;
    let end = text.find('\n').unwrap() + 1;
    text.replace_range(..end, &header(data, &id, version));
    Ok(text)
}

fn header(data: &Dataset, artifact: &str, version: Version) -> String {
    match version {
        Version::V1 => format!(
            "[scopelet compact-v1 {artifact} scan_complete={}; original: scopelet expand {artifact} --manifest]\n",
            data.scan_complete
        ),
        Version::V2 => format!(
            "[scopelet compact-v2 {artifact} scan_complete={}; recover: scopelet expand {artifact} --find TEXT (or --manifest)]\n",
            data.scan_complete
        ),
    }
}
fn footer(omitted: usize) -> String {
    format!(
        "[scopelet display_complete={} omitted_units={omitted}; gaps are omitted, not evidence of absence]\n",
        omitted == 0
    )
}

struct Unit<'a> {
    record: &'a Record,
    line: Option<&'a str>,
    start: usize,
    end: usize,
    size: usize,
    priority: u8,
}
fn digits(n: usize) -> usize {
    if n == 0 { 1 } else { n.ilog10() as usize + 1 }
}
impl Unit<'_> {
    fn append(&self, output: &mut String) {
        if let Some(line) = self.line {
            if self.end == self.start + 1 {
                output.push_str(&format!("{}:{} ", self.record.source, self.start));
            } else {
                output.push_str(&format!(
                    "{}:{}-{} repeat={} ",
                    self.record.source,
                    self.start,
                    self.end - 1,
                    self.end - self.start
                ));
            }
            output.push_str(line);
            if !line.ends_with('\n') {
                output.push('\n');
            }
        } else {
            output.push_str(&self.record.source);
            output.push_str(": ");
            output.push_str(&self.record.value.as_ref().unwrap().to_string());
            output.push('\n');
        }
    }
}

fn signal_value(value: &serde_json::Value) -> Option<&str> {
    match value {
        serde_json::Value::String(s) => SIGNAL.is_match(s).then_some(s),
        serde_json::Value::Array(a) => a.iter().find_map(signal_value),
        serde_json::Value::Object(o) => o.values().find_map(signal_value),
        _ => None,
    }
}

fn units(data: &Dataset, limit: usize, version: Version) -> Vec<Unit<'_>> {
    let mut units = Vec::new();
    let mut distinct = BTreeSet::new();
    for (ordinal, record) in data.records.iter().enumerate() {
        if let Some(value) = &record.value {
            let priority = if version == Version::V1 {
                if SIGNAL.is_match(&value.to_string()) {
                    3
                } else {
                    0
                }
            } else if ordinal == 0 || ordinal + 1 == data.records.len() {
                4
            } else if let Some(signal) = signal_value(value) {
                if distinct.insert(signal) { 3 } else { 2 }
            } else {
                0
            };
            units.push(Unit {
                record,
                line: None,
                start: 0,
                end: 0,
                size: encoding::size(value, limit).unwrap_or(limit + 1) + record.source.len() + 3,
                priority,
            });
            continue;
        }
        let lines: Vec<_> = record.text.split_inclusive('\n').collect();
        let mut nearby = vec![false; lines.len()];
        let signals: Vec<_> = lines.iter().map(|line| SIGNAL.is_match(line)).collect();
        for (i, &signal) in signals.iter().enumerate() {
            if signal {
                nearby[i.saturating_sub(3)..(i + 9).min(lines.len())].fill(true);
            }
        }
        let front = lines.len().min(3);
        nearby[..front].fill(true);
        nearby[lines.len().saturating_sub(5)..].fill(true);
        let base = record.start_line.unwrap_or(1);
        let mut i = 0;
        while i < lines.len() {
            let mut end = i + 1;
            while end < lines.len() && lines[end] == lines[i] {
                end += 1;
            }
            let line = lines[i];
            let priority = if i == 0 || end == lines.len() {
                4
            } else if signals[i] {
                if version == Version::V1 || distinct.insert(line) {
                    3
                } else {
                    2
                }
            } else if nearby[i..end].iter().any(|&v| v) {
                1
            } else {
                0
            };
            let start_line = base + i;
            let end_line = base + end;
            let range_size = digits(start_line)
                + if end == i + 1 {
                    0
                } else {
                    1 + digits(end_line - 1) + 8 + digits(end - i)
                };
            let size = record.source.len()
                + 2
                + range_size
                + line.len()
                + usize::from(!line.ends_with('\n'));
            units.push(Unit {
                record,
                line: Some(line),
                start: start_line,
                end: end_line,
                size,
                priority,
            });
            i = end;
        }
    }
    units
}

fn select(units: &[Unit<'_>], available: usize) -> Vec<usize> {
    let mut selected = vec![false; units.len()];
    let mut used = 0;
    for priority in [4, 3, 2, 1, 0] {
        for (i, unit) in units.iter().enumerate() {
            if unit.priority == priority && unit.size <= available.saturating_sub(used) {
                selected[i] = true;
                used += unit.size;
            }
        }
    }
    selected
        .into_iter()
        .enumerate()
        .filter_map(|(i, yes)| yes.then_some(i))
        .collect()
}

fn view(
    data: &Dataset,
    artifact: &str,
    budget: usize,
    version: Version,
) -> Result<(String, usize)> {
    ensure!(
        (512..=1024 * 1024).contains(&budget),
        "internal compact budget must be 512..1048576"
    );
    let count = data
        .records
        .iter()
        .try_fold(0usize, |n, r| {
            n.checked_add(if r.value.is_some() {
                1
            } else {
                r.text.lines().count()
            })
        })
        .unwrap_or(usize::MAX);
    ensure!(
        count <= 100_000,
        "compact input exceeds 100000 units; use an explicit query"
    );
    let mut output = header(data, artifact, version);
    let available = budget.saturating_sub(output.len() + 160);
    let uniform = compact_table::uniform(data);
    if uniform {
        let indices: Vec<_> = (0..data.records.len()).collect();
        let tail =
            "[scopelet display_complete=true omitted_units=0; rows map positionally to columns]\n";
        if let Some(table) = compact_table::encode(
            data,
            &indices,
            version == Version::V2,
            budget.saturating_sub(output.len() + tail.len()),
        ) {
            output.push_str(&table);
            output.push_str(tail);
            return Ok((output, indices.len()));
        }
    }
    let mut units = units(data, available, version);
    if uniform && version == Version::V2 {
        // Exact row costs include positional provenance. Fixed schema/envelope costs
        // are counted by the same serializer as the final table.
        if let Some(empty) = compact_table::encode(data, &[], true, available) {
            let varying_sources = data
                .records
                .iter()
                .any(|r| r.source != data.records[0].source);
            for (i, unit) in units.iter_mut().enumerate() {
                unit.size = compact_table::row_size(unit.record, available)
                    .unwrap_or(available + 1)
                    + digits(i)
                    + 2
                    + if varying_sources {
                        encoding::size(&unit.record.source, available).unwrap_or(available + 1) + 1
                    } else {
                        0
                    };
            }
            let indices = select(&units, available.saturating_sub(empty.len()));
            if !indices.is_empty()
                && let Some(table) = compact_table::encode(data, &indices, true, available)
            {
                output.push_str(&table);
                output.push_str(&footer(units.len() - indices.len()));
                return Ok((output, indices.len()));
            }
        }
        // No table row fits: recompute ordinary-record costs before falling back.
        units = self::units(data, available, version);
    }
    let selected = select(&units, available);
    for &i in &selected {
        units[i].append(&mut output);
    }
    output.push_str(&footer(units.len() - selected.len()));
    ensure!(output.len() <= budget, "compact metadata exceeds budget");
    Ok((output, selected.len()))
}
