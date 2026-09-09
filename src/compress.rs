//! Recoverable presentation for automatic integrations. Never rewrites originals.
use crate::{model::*, sources, store::Store};
use anyhow::{Result, ensure};
use std::collections::BTreeSet;
use std::sync::LazyLock;

pub const DEFAULT_BUDGET: usize = 4096;
pub const SMALL: usize = 2048;
static SIGNAL: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(error|failed|failure|panic|panicked|exception|traceback|warning|assertionerror|caused by|test result|tests? passed|tests? failed)\b|^\s*(FAIL|PASS|E\s+|FATAL|×|✕)").unwrap()
});

/// Conservative detection: malformed structured input is never partly aggregated.
pub fn detect(text: &str) -> Format {
    if serde_json::from_str::<serde_json::Value>(text).is_ok() {
        Format::Json
    } else if text.lines().count() > 1
        && text
            .lines()
            .all(|line| serde_json::from_str::<serde_json::Value>(line).is_ok())
    {
        Format::Jsonl
    } else {
        Format::Text
    }
}

/// Return original bytes unless the entire replacement is materially smaller.
pub fn automatic(bytes: &[u8], store: &Store, budget: usize) -> Result<Vec<u8>> {
    ensure!(
        (1024..=1024 * 1024).contains(&budget),
        "max_bytes must be 1024..1048576"
    );
    if bytes.len() <= SMALL || bytes.iter().filter(|&&b| b == b'\n').count() > 100_000 {
        return Ok(bytes.to_vec());
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Ok(bytes.to_vec());
    };
    if text.contains("[scopelet ") || text.contains("<persisted-output>") {
        return Ok(bytes.to_vec());
    }
    let mut data = Dataset::default();
    sources::ingest(
        &mut data,
        store,
        "input",
        bytes.to_vec(),
        None,
        detect(text),
    )?;
    data.notes
        .push("Bytes supplied to the compressor; completeness before capture is unknown.".into());
    let (result, shown) = compact_view(&data, store, budget)?;
    if shown > 0 && result.len() + 512 <= bytes.len() && result.len() * 5 <= bytes.len() * 4 {
        Ok(result.into_bytes())
    } else {
        Ok(bytes.to_vec())
    }
}

/// Factor a uniform object schema without dropping or coercing any value.
/// Heterogeneous keys (including missing versus null) use ordinary records instead.
fn table(data: &Dataset) -> Option<String> {
    let first = data.records.first()?;
    let object = first.value.as_ref()?.as_object()?;
    if data.records.len() < 2 {
        return None;
    }
    let columns: Vec<_> = object.keys().cloned().collect();
    let mut rows = Vec::new();
    for record in &data.records {
        if record.blob != first.blob || (record.blob.is_none() && record.source != first.source) {
            return None;
        }
        let row = record.value.as_ref()?.as_object()?;
        if row.keys().ne(object.keys()) {
            return None;
        }
        rows.push(
            columns
                .iter()
                .map(|key| row[key].clone())
                .collect::<Vec<_>>(),
        );
    }
    let mut value = serde_json::json!({"source":first.source,"columns":columns,"rows":rows});
    if data.records.iter().any(|r| r.source != first.source) {
        value["record_sources"] =
            serde_json::json!(data.records.iter().map(|r| &r.source).collect::<Vec<_>>());
    }
    Some(format!("table {value}\n"))
}

/// Compact presentation has its own envelope; the version-1 JSON view is unchanged.
pub fn compact(data: &Dataset, store: &Store, budget: usize) -> Result<String> {
    compact_view(data, store, budget).map(|(text, _)| text)
}

fn compact_view(data: &Dataset, store: &Store, budget: usize) -> Result<(String, usize)> {
    ensure!(
        (512..=1024 * 1024).contains(&budget),
        "internal compact budget must be 512..1048576"
    );
    let units = data
        .records
        .iter()
        .try_fold(0usize, |total, record| {
            let count = if record.value.is_some() {
                1
            } else {
                record.text.lines().count()
            };
            total.checked_add(count)
        })
        .unwrap_or(usize::MAX);
    ensure!(
        units <= 100_000,
        "compact input exceeds 100000 units; use an explicit query"
    );
    let artifact = store.put("artifact", &serde_json::to_vec(data)?)?;
    let header = format!(
        "[scopelet compact-v1 {artifact} scan_complete={}; original: scopelet expand {artifact} --manifest]\n",
        data.scan_complete
    );
    // Reserve the footer even when all records fit.
    let available = budget.saturating_sub(header.len() + 160);
    if let Some(table) = table(data) {
        let complete = format!(
            "{header}{table}[scopelet display_complete=true omitted_units=0; rows map positionally to columns]\n"
        );
        if complete.len() <= budget {
            return Ok((complete, data.records.len()));
        }
    }
    let mut units: Vec<(String, u8)> = Vec::new();
    for record in &data.records {
        if let Some(value) = &record.value {
            let value = value.to_string();
            units.push((
                format!("{}: {value}\n", record.source),
                if SIGNAL.is_match(&value) { 3 } else { 0 },
            ));
        } else {
            let lines: Vec<_> = record.text.split_inclusive('\n').collect();
            let mut priority = BTreeSet::new();
            priority.extend(0..lines.len().min(3));
            priority.extend(lines.len().saturating_sub(5)..lines.len());
            for (index, line) in lines.iter().enumerate() {
                if SIGNAL.is_match(line) {
                    priority.extend(index.saturating_sub(3)..(index + 9).min(lines.len()));
                }
            }
            let base = record.start_line.unwrap_or(1);
            let mut index = 0;
            while index < lines.len() {
                let mut end = index + 1;
                while end < lines.len() && lines[end] == lines[index] {
                    end += 1;
                }
                let range = if end == index + 1 {
                    format!("{}", base + index)
                } else {
                    format!("{}-{} repeat={}", base + index, base + end - 1, end - index)
                };
                let line = lines[index];
                units.push((
                    format!(
                        "{}:{range} {line}{}",
                        record.source,
                        if line.ends_with('\n') { "" } else { "\n" }
                    ),
                    if index == 0 || end == lines.len() {
                        4
                    } else if SIGNAL.is_match(line) {
                        3
                    } else if (index..end).any(|i| priority.contains(&i)) {
                        1
                    } else {
                        0
                    },
                ));
                index = end;
            }
        }
    }
    let mut selected = BTreeSet::new();
    let mut used = 0;
    // Diagnostics first; output remains in source order. Whole lines/JSON records only.
    for priority in [4, 3, 1, 0] {
        for (index, (unit, important)) in units.iter().enumerate() {
            if *important == priority && used + unit.len() <= available {
                selected.insert(index);
                used += unit.len();
            }
        }
    }
    let omitted = units.len() - selected.len();
    let mut result = header;
    for index in selected {
        result.push_str(&units[index].0);
    }
    result.push_str(&format!("[scopelet display_complete={} omitted_units={omitted}; gaps are omitted, not evidence of absence]\n", omitted == 0));
    ensure!(result.len() <= budget, "compact metadata exceeds budget");
    Ok((result, units.len() - omitted))
}
