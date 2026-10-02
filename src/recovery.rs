//! Explicit recovery searches immutable originals, never the current filesystem.
use crate::{model::*, render::View, search::Search, store::Store};
use anyhow::{Context, Result, ensure};
use std::collections::BTreeSet;

/// `regex` treats patterns as regular expressions instead of literal text;
/// `ignore_case` matches without regard to case.
pub fn search(
    store: &Store,
    id: &str,
    patterns: &[String],
    (regex, ignore_case): (bool, bool),
    context: usize,
    source: Option<&str>,
) -> Result<Dataset> {
    let search = Search::new(patterns, false, regex, ignore_case, context)?;
    let mut data = if id.starts_with("artifact:") {
        // Only the snapshots are searched: records are not parsed again.
        let data = store.dataset_head(id)?;
        store.touch(id);
        data
    } else {
        let bytes = store.get(id)?;
        store.touch(id);
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
        store.touch(&snapshot.blob);
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

/// Lines `start..=end` of a blob, or as many of them as fit `budget`: a view
/// that cannot show them all shows the first ones and sets `next_start`.
/// Only a single line larger than the budget is still a blocked record.
pub fn page(store: &Store, id: &str, start: usize, end: usize, budget: usize) -> Result<View> {
    const NOTE: &str =
        "Lines that did not fit were not shown; continue with expand --start next_start.";
    let data = range(store, id, start, end)?;
    // Size candidates without storing them: the placeholder has the length
    // of a real artifact reference.
    let placeholder = format!("artifact:{}", "0".repeat(64));
    let measure = |data: &Dataset, next: Option<usize>| -> Option<View> {
        let mut view =
            crate::render::render_stored(data, placeholder.clone(), Mode::Default, budget, 0)
                .ok()?;
        if view.blocked_record.is_some() {
            return None;
        }
        if next.is_some() {
            view.next_start = next;
            view.display_complete = false;
            view.notes.push(NOTE.into());
        }
        (serde_json::to_vec(&view).ok()?.len() < budget).then_some(view)
    };
    if data.records.is_empty() || measure(&data, None).is_some() {
        return crate::render::render(&data, store, Mode::Default, budget, 0);
    }
    let record = &data.records[0];
    let lines: Vec<&str> = record.text.split_inclusive('\n').collect();
    let first = record.start_line.unwrap_or(start);
    let take = |count: usize| Dataset {
        records: vec![Record {
            text: lines[..count].concat(),
            end_line: Some(first + count - 1),
            ..record.clone()
        }],
        ..data.clone()
    };
    // The largest prefix of whole lines that fits, by bisection.
    let (mut fits, mut over) = (0, lines.len());
    while over - fits > 1 {
        let mid = (fits + over) / 2;
        if measure(&take(mid), Some(first + mid)).is_some() {
            fits = mid;
        } else {
            over = mid;
        }
    }
    if fits == 0 {
        // A single line larger than the budget: report it as blocked.
        return crate::render::render(&data, store, Mode::Default, budget, 0);
    }
    let mut view = crate::render::render(&take(fits), store, Mode::Default, budget, 0)?;
    view.next_start = Some(first + fits);
    view.display_complete = false;
    view.notes.push(NOTE.into());
    Ok(view)
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
    store.touch(id);
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
