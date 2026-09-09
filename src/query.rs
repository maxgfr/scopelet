//! Conservative execution planning. Global operations retain the materialized path.
use crate::{
    model::*,
    pipeline,
    search::Search,
    sources,
    store::{MAX_INPUT, Store, read_bounded},
};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

pub fn execute(request: &Request, store: &Store, cancel: Arc<AtomicBool>) -> Result<Dataset> {
    pipeline::validate(request)?;
    if let Source::File {
        path,
        format: Format::Jsonl,
    } = &request.source
        && streamable(&request.operations)
    {
        return aggregate(path, &request.operations, store);
    }
    if let Source::Repo { .. } = request.source
        && let Some(Operation::Search {
            patterns,
            all,
            regex,
            context,
        }) = request.operations.first()
    {
        // For invalid searches retain source-loading/error precedence of the old path.
        if let Ok(search) = Search::new(patterns, *all, *regex, *context) {
            let mut data = sources::load_search(&request.source, store, cancel, Some(&search))?;
            pipeline::apply(&mut data, &request.operations[1..])?;
            return Ok(data);
        }
    }
    let mut data = sources::load(&request.source, store, cancel)?;
    pipeline::apply(&mut data, &request.operations)?;
    Ok(data)
}
fn valid_pointer(p: &str) -> bool {
    p.is_empty() || p.starts_with('/')
}
fn streamable(operations: &[Operation]) -> bool {
    let Some((last, filters)) = operations.split_last() else {
        return false;
    };
    (matches!(last, Operation::Count)
        || matches!(last, Operation::Group { pointer } if valid_pointer(pointer)))
        && filters
            .iter()
            .all(|op| matches!(op, Operation::Filter { pointer, .. } if valid_pointer(pointer)))
}
fn aggregate(path: &str, operations: &[Operation], store: &Store) -> Result<Dataset> {
    let path = Path::new(path).canonicalize()?;
    let name = path.to_string_lossy().into_owned();
    let raw = read_bounded(&path, MAX_INPUT)?;
    let blob = store.put("blob", &raw)?;
    let text = std::str::from_utf8(&raw)
        .context("text and JSON sources must be UTF-8; original bytes are saved in cache")?;
    let (last, filters) = operations.split_last().unwrap();
    let mut count = 0;
    let mut groups: BTreeMap<String, (Value, usize)> = BTreeMap::new();
    let mut missing = None;
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line).with_context(|| {
            format!(
                "malformed JSONL at {name}:{}; no partial aggregate emitted",
                i + 1
            )
        })?;
        if !filters.iter().all(|op| match op {
            Operation::Filter { pointer, equals } => value.pointer(pointer) == Some(equals),
            _ => unreachable!(),
        }) {
            continue;
        }
        count += 1;
        if let Operation::Group { pointer } = last {
            if let Some(v) = value.pointer(pointer) {
                let key = v.to_string();
                if let Some((_, count)) = groups.get_mut(&key) {
                    *count += 1;
                } else {
                    groups.insert(key, (v.clone(), 1));
                }
            } else if missing.is_none() {
                missing = Some(format!("missing JSON pointer {pointer:?} in {name}"));
            }
        }
    }
    // Parse the entire source before surfacing an operation error, just as load + apply.
    if let Some(error) = missing {
        anyhow::bail!(error);
    }
    Ok(Dataset {
        examined: 1,
        snapshots: vec![Snapshot {
            source: name.clone(),
            blob,
            bytes: raw.len(),
            local_path: Some(name),
        }],
        records: match last {
            Operation::Count => vec![Record::derived(json!({"count":count}))],
            _ => groups
                .into_values()
                .map(|(key, count)| Record::derived(json!({"key":key,"count":count})))
                .collect(),
        },
        ..Dataset::default()
    })
}
