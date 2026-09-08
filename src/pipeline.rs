use crate::model::{Dataset, Operation, Record};
use anyhow::{Result, bail, ensure};
use regex::RegexBuilder;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub fn apply(data: &mut Dataset, operations: &[Operation]) -> Result<()> {
    ensure!(operations.len() <= 32, "at most 32 operations per request");
    for op in operations {
        data.records = transform(std::mem::take(&mut data.records), op)?;
    }
    Ok(())
}

fn pointer<'a>(record: &'a Record, path: &str) -> Result<&'a Value> {
    ensure!(
        path.is_empty() || path.starts_with('/'),
        "use a JSON Pointer, e.g. /status"
    );
    record
        .value
        .as_ref()
        .and_then(|v| v.pointer(path))
        .ok_or_else(|| anyhow::anyhow!("missing JSON pointer {path:?} in {}", record.source))
}

fn transform(records: Vec<Record>, op: &Operation) -> Result<Vec<Record>> {
    match op {
        Operation::Search {
            patterns,
            all,
            regex,
            context,
        } => {
            ensure!(
                !patterns.is_empty() && patterns.len() <= 64,
                "provide 1..64 search patterns"
            );
            ensure!(*context <= 1000, "context must be <= 1000 lines");
            let patterns = patterns
                .iter()
                .map(|p| {
                    RegexBuilder::new(&if *regex { p.clone() } else { regex::escape(p) })
                        .multi_line(true)
                        .crlf(true)
                        .build()
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let mut out = Vec::new();
            for mut r in records {
                let content = r.searchable();
                let matched = if *all {
                    patterns.iter().all(|p| p.is_match(&content))
                } else {
                    patterns.iter().any(|p| p.is_match(&content))
                };
                if !matched {
                    continue;
                }
                // Structured records remain whole; never join fields from different objects.
                if r.value.is_some()
                    || patterns
                        .iter()
                        .any(|p| p.find_iter(&content).any(|m| m.as_str().contains('\n')))
                {
                    out.push(r);
                    continue;
                }
                let text = std::mem::take(&mut r.text);
                let lines: Vec<&str> = text.split_inclusive('\n').collect();
                let mut spans: Vec<(usize, usize)> = Vec::new();
                for (i, line) in lines.iter().enumerate() {
                    if patterns.iter().any(|p| p.is_match(line)) {
                        let start = i.saturating_sub(*context);
                        let end = (i + context + 1).min(lines.len());
                        if let Some(last) = spans.last_mut().filter(|last| start <= last.1) {
                            last.1 = last.1.max(end);
                        } else {
                            spans.push((start, end));
                        }
                    }
                }
                // Multiline regex can match across physical lines. Preserve the whole unit.
                if spans.is_empty() {
                    r.text = text;
                    out.push(r);
                    continue;
                }
                for (start, end) in spans {
                    let base = r.start_line.unwrap_or(1);
                    out.push(Record {
                        text: lines[start..end].concat(),
                        start_line: Some(base + start),
                        end_line: Some(base + end - 1),
                        ..r.clone()
                    });
                }
            }
            Ok(out)
        }
        Operation::Filter {
            pointer: path,
            equals,
        } => {
            ensure!(
                path.is_empty() || path.starts_with('/'),
                "use a JSON Pointer"
            );
            let mut out = Vec::new();
            for record in records {
                let value = record
                    .value
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("filter requires JSON records"))?;
                // Missing differs from explicit null, as in RFC 6901.
                if value.pointer(path) == Some(equals) {
                    out.push(record);
                }
            }
            Ok(out)
        }
        Operation::Project { pointers } => {
            ensure!(!pointers.is_empty(), "project needs at least one pointer");
            records
                .into_iter()
                .map(|mut r| {
                    let mut value = serde_json::Map::new();
                    for p in pointers {
                        value.insert(p.clone(), pointer(&r, p)?.clone());
                    }
                    r.value = Some(Value::Object(value));
                    r.text.clear();
                    Ok(r)
                })
                .collect()
        }
        Operation::Count => Ok(vec![Record::derived(json!({"count": records.len()}))]),
        Operation::Group { pointer: path } => {
            let mut groups: BTreeMap<String, (Value, usize)> = BTreeMap::new();
            for record in &records {
                let v = pointer(record, path)?.clone();
                let (_, count) = groups.entry(v.to_string()).or_insert((v, 0));
                *count += 1;
            }
            Ok(groups
                .into_values()
                .map(|(value, count)| Record::derived(json!({"key": value, "count": count})))
                .collect())
        }
        Operation::Unique => {
            let mut seen = BTreeSet::new();
            Ok(records
                .into_iter()
                .filter(|r| seen.insert(r.searchable()))
                .collect())
        }
        Operation::Read { start, end } => {
            ensure!(
                *start >= 1 && end >= start,
                "read uses inclusive 1-based start/end"
            );
            let mut out = Vec::new();
            for mut r in records {
                ensure!(r.value.is_none(), "read is for text; use project for JSON");
                let base = r.start_line.unwrap_or(1);
                let lines: Vec<&str> = r.text.split_inclusive('\n').collect();
                let lo = start.saturating_sub(base).min(lines.len());
                let hi = end.saturating_add(1).saturating_sub(base).min(lines.len());
                if lo < hi {
                    r.text = lines[lo..hi].concat();
                    r.start_line = Some(base + lo);
                    r.end_line = Some(base + hi - 1);
                    out.push(r);
                }
            }
            Ok(out)
        }
        Operation::Rank { query } => {
            ensure!(!query.trim().is_empty(), "rank needs a query");
            let terms: BTreeSet<String> = query.split_whitespace().map(str::to_lowercase).collect();
            let mut ranked: Vec<(usize, usize, Record)> = records
                .into_iter()
                .enumerate()
                .map(|(i, r)| {
                    let text = format!("{} {}", r.source, r.searchable()).to_lowercase();
                    let score = terms.iter().filter(|t| text.contains(t.as_str())).count();
                    (score, i, r)
                })
                .collect();
            ranked.sort_by_key(|(score, i, _)| (std::cmp::Reverse(*score), *i));
            Ok(ranked.into_iter().map(|(_, _, r)| r).collect())
        }
    }
}

pub fn validate(request: &crate::model::Request) -> Result<()> {
    ensure!(
        request.version == 1,
        "unsupported request version {}; expected 1",
        request.version
    );
    if let Some(bytes) = request.max_bytes {
        ensure!(
            (1024..=1024 * 1024).contains(&bytes),
            "max_bytes must be 1024..1048576"
        );
    }
    if request.operations.len() > 32 {
        bail!("at most 32 operations");
    }
    Ok(())
}
