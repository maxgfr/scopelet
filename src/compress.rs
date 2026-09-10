//! Recoverable presentation. Selection borrows evidence; storage follows acceptance.
use crate::{clean, compact_table, encoding, model::*, sources::Parsed, store::Store};
use anyhow::{Result, ensure};
use std::{
    borrow::Cow,
    collections::{BTreeSet, HashSet},
    path::PathBuf,
    sync::LazyLock,
};

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
    #[value(name = "2")]
    V2,
    #[default]
    #[value(name = "3")]
    V3,
}
impl Version {
    pub fn configured(explicit: Option<Self>) -> Result<Self> {
        if let Some(version) = explicit {
            return Ok(version);
        }
        match std::env::var("SCOPELET_COMPACT_VERSION") {
            Ok(v) if v == "1" => Ok(Self::V1),
            Ok(v) if v == "2" => Ok(Self::V2),
            Ok(v) if v == "3" => Ok(Self::V3),
            Err(std::env::VarError::NotPresent) => Ok(Self::default()),
            _ => anyhow::bail!("SCOPELET_COMPACT_VERSION must be 1, 2 or 3"),
        }
    }
    /// Partial JSON tables with indices and provenance (everything after v1).
    fn tables(self) -> bool {
        self != Self::V1
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
        // ID refers to the artifact reference above: the id is not repeated.
        Version::V3 => format!(
            "[scopelet compact-v3 {artifact} scan_complete={}; recover: scopelet expand ID --find TEXT (or --manifest)]\n",
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
    /// Displayed text of a text unit; JSON units serialize their value.
    line: Option<Cow<'a, str>>,
    /// Label and metadata before the text, including the separating space.
    prefix: String,
    size: usize,
    priority: u8,
}
fn digits(n: usize) -> usize {
    if n == 0 { 1 } else { n.ilog10() as usize + 1 }
}
impl<'a> Unit<'a> {
    fn text(record: &'a Record, prefix: String, line: Cow<'a, str>, priority: u8) -> Self {
        let size = prefix.len() + line.len() + usize::from(!line.ends_with('\n'));
        Self {
            record,
            line: Some(line),
            prefix,
            size,
            priority,
        }
    }
    /// Compact-v3 text unit; a line too large for the budget is cut on a
    /// character boundary behind a `text_truncated bytes=N` marker.
    fn text_v3(
        record: &'a Record,
        label: String,
        view: Cow<'a, str>,
        raw: &str,
        priority: u8,
        limit: usize,
    ) -> Self {
        let size = label.len() + 1 + view.len() + 1;
        let cap = limit.min(1024);
        let prefix = format!("{label} text_truncated bytes={} ", clean::body(raw).len());
        if size <= limit || prefix.len() + 1 >= cap {
            return Self::text(record, label + " ", view, priority);
        }
        let cut = (0..=cap - prefix.len() - 1)
            .rev()
            .find(|&i| view.is_char_boundary(i))
            .unwrap();
        Self {
            record,
            line: Some(Cow::Owned(view[..cut].to_owned())),
            size: prefix.len() + cut + 1,
            prefix,
            priority,
        }
    }
    fn append(&self, output: &mut String) {
        if let Some(line) = &self.line {
            output.push_str(&self.prefix);
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
    let mut seen = HashSet::new();
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
                prefix: String::new(),
                size: encoding::size(value, limit).unwrap_or(limit + 1) + record.source.len() + 3,
                priority,
            });
            continue;
        }
        if version == Version::V3 {
            text_units_v3(record, limit, &mut seen, &mut units);
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
            let prefix = if end == i + 1 {
                format!("{}:{} ", record.source, base + i)
            } else {
                format!(
                    "{}:{}-{} repeat={} ",
                    record.source,
                    base + i,
                    base + end - 1,
                    end - i
                )
            };
            units.push(Unit::text(record, prefix, Cow::Borrowed(line), priority));
            i = end;
        }
    }
    units
}

/// Compact-v3 text units work on display copies of the lines (terminal
/// control sequences and carriage-return overwrites removed) while labels stay
/// absolute source lines; oversized lines are cut instead of vetoing the view.
fn text_units_v3<'a>(
    record: &'a Record,
    limit: usize,
    seen: &mut HashSet<String>,
    units: &mut Vec<Unit<'a>>,
) {
    let lines: Vec<&str> = record.text.split_inclusive('\n').collect();
    let views: Vec<Cow<'a, str>> = lines.iter().map(|line| clean::line_view(line)).collect();
    let signals: Vec<bool> = views.iter().map(|view| SIGNAL.is_match(view)).collect();
    let mut nearby = vec![false; lines.len()];
    for (i, &signal) in signals.iter().enumerate() {
        if signal {
            nearby[i.saturating_sub(3)..(i + 9).min(lines.len())].fill(true);
        }
    }
    nearby[..lines.len().min(3)].fill(true);
    nearby[lines.len().saturating_sub(5)..].fill(true);
    let base = record.start_line.unwrap_or(1);
    let mut i = 0;
    while i < lines.len() {
        let mut end = i + 1;
        while end < lines.len() && views[end] == views[i] {
            end += 1;
        }
        let priority = if i == 0 || end == lines.len() {
            4
        } else if signals[i] {
            if seen.insert(views[i].to_string()) {
                3
            } else {
                2
            }
        } else if nearby[i..end].iter().any(|&v| v) {
            1
        } else {
            0
        };
        let label = if end == i + 1 {
            format!("{}:{}", record.source, base + i)
        } else {
            format!(
                "{}:{}-{} repeat={}",
                record.source,
                base + i,
                base + end - 1,
                end - i
            )
        };
        units.push(Unit::text_v3(
            record,
            label,
            views[i].clone(),
            lines[i],
            priority,
            limit,
        ));
        i = end;
    }
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
    // V3 reserves exactly the widest footer it can emit; earlier versions keep
    // their fixed reserve so their bytes stay identical.
    let reserve = if version == Version::V3 {
        footer(count).len()
    } else {
        160
    };
    let available = budget.saturating_sub(output.len() + reserve);
    let uniform = compact_table::uniform(data);
    if uniform {
        let indices: Vec<_> = (0..data.records.len()).collect();
        let tail =
            "[scopelet display_complete=true omitted_units=0; rows map positionally to columns]\n";
        if let Some(table) = compact_table::encode(
            data,
            &indices,
            version.tables(),
            budget.saturating_sub(output.len() + tail.len()),
        ) {
            output.push_str(&table);
            output.push_str(tail);
            return Ok((output, indices.len()));
        }
    }
    let mut units = units(data, available, version);
    if uniform && version.tables() {
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
