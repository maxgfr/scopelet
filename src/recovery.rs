//! Explicit recovery searches immutable originals, never the current filesystem.
use crate::{model::*, search::Search, store::Store};
use anyhow::{Context, Result, ensure};
use std::collections::BTreeSet;

pub fn search(
    store: &Store,
    id: &str,
    patterns: &[String],
    context: usize,
    source: Option<&str>,
) -> Result<Dataset> {
    let search = Search::new(patterns, false, false, context)?;
    let bytes = store.get(id)?;
    store.touch(id)?;
    let mut data = if id.starts_with("artifact:") {
        let mut data: Dataset =
            serde_json::from_slice(&bytes).context("invalid dataset artifact")?;
        ensure!(data.schema_version == 1, "unsupported artifact schema");
        data.records.clear();
        data
    } else {
        ensure!(source.is_none(), "--source requires an artifact reference");
        Dataset {
            snapshots: vec![Snapshot {
                source: id.into(),
                blob: id.into(),
                bytes: bytes.len(),
                local_path: None,
            }],
            ..Dataset::default()
        }
    };
    if let Some(source) = source {
        ensure!(
            data.snapshots.iter().any(|s| s.source == source),
            "unknown source label {source:?}"
        );
        data.snapshots.retain(|s| s.source == source);
    }
    let mut seen = BTreeSet::new();
    for snapshot in &data.snapshots {
        if !seen.insert((&snapshot.source, &snapshot.blob)) {
            continue;
        }
        let raw = store.get(&snapshot.blob)?;
        store.touch(&snapshot.blob)?;
        let text =
            String::from_utf8(raw).context("saved source is not UTF-8; expand --raw instead")?;
        let record = Record {
            source: snapshot.source.clone(),
            blob: Some(snapshot.blob.clone()),
            start_line: Some(1),
            end_line: Some(text.lines().count()),
            text,
            value: None,
            omitted_lines: None,
            text_truncated: false,
        };
        data.records.extend(search.apply(vec![record]));
    }
    data.notes.push("Immutable original snapshot search; does not assert the sources are still current. Matches are text windows, not structured JSON records.".into());
    Ok(data)
}

/// Range recovery uses a disposable offset index after verifying the full blob.
pub fn range(store: &Store, id: &str, start: usize, end: usize) -> Result<Dataset> {
    ensure!(
        start >= 1 && end >= start,
        "read uses inclusive 1-based start/end"
    );
    ensure!(
        id.starts_with("blob:"),
        "line ranges require a blob reference"
    );
    let bytes = store.get(id)?;
    store.touch(id)?;
    let text =
        String::from_utf8(bytes).context("saved source is not UTF-8; expand --raw instead")?;
    let (selected, count) = crate::line_index::slice(store, id, &text, start, end);
    let records = if count == 0 {
        vec![]
    } else {
        vec![Record {
            source: id.into(),
            blob: Some(id.into()),
            start_line: Some(start),
            end_line: Some(start + count - 1),
            text: selected,
            value: None,
            omitted_lines: None,
            text_truncated: false,
        }]
    };
    Ok(Dataset {
        snapshots: vec![Snapshot {
            source: id.into(),
            blob: id.into(),
            bytes: text.len(),
            local_path: None,
        }],
        records,
        notes: vec![
            "Immutable original snapshot; does not assert the source is still current.".into(),
        ],
        ..Dataset::default()
    })
}
