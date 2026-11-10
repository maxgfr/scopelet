//! Explicit recovery searches immutable originals, never the current filesystem.
use crate::{
    model::*,
    render::View,
    search::Search,
    store::{MAX_INPUT, Store, digest, read_bounded},
};
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
    // A blob searched by its own reference is read and verified once: the
    // bytes that gave its length are the ones searched below.
    let mut read = None;
    let mut data = if id.starts_with("artifact:") {
        // Only the snapshots are searched: records are not parsed again.
        let data = store.dataset_head(id)?;
        store.touch(id);
        data
    } else {
        ensure!(source.is_none(), "--source requires an artifact reference");
        let bytes = store.get(id)?;
        store.touch(id);
        let snapshot = Snapshot {
            source: id.into(),
            blob: id.into(),
            bytes: bytes.len(),
            local_path: None,
        };
        read = Some(bytes);
        Dataset {
            snapshots: vec![snapshot],
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
    let mut not_retained = 0;
    let mut not_text = 0;
    for snapshot in &data.snapshots {
        if !seen.insert((&snapshot.source, &snapshot.blob)) {
            continue;
        }
        let raw = if let Some(raw) = read.take() {
            raw
        } else if store.contains(&snapshot.blob) {
            let raw = store.get(&snapshot.blob)?;
            store.touch(&snapshot.blob);
            raw
        } else {
            // A repository scan keeps only the originals of files that
            // matched; the local file serves while it still has those bytes.
            let current = snapshot
                .local_path
                .as_deref()
                .and_then(|path| read_bounded(std::path::Path::new(path), MAX_INPUT).ok())
                .filter(|raw| snapshot.blob.ends_with(&digest(raw)));
            match current {
                Some(raw) => raw,
                None => {
                    not_retained += 1;
                    continue;
                }
            }
        };
        let text = match String::from_utf8(raw) {
            Ok(text) => text,
            // An artifact can hold an original that is not text beside the
            // text extracted from it (a PDF and its extraction share one
            // label): the search covers the text and counts the other,
            // instead of failing on it. A blob searched by itself still fails.
            Err(_) if id.starts_with("artifact:") => {
                not_text += 1;
                continue;
            }
            Err(error) => {
                return Err(error).context("saved source is not UTF-8; expand --raw instead");
            }
        };
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
    if not_text > 0 {
        for _ in 0..not_text {
            data.skip("original_not_text");
        }
        data.notes.push("An original that is not UTF-8 text (a document's own bytes) was not searched: its extracted text was. Read it with expand BLOB --raw.".into());
    }
    if not_retained > 0 {
        for _ in 0..not_retained {
            data.skip("snapshot_not_retained");
        }
        data.incomplete("Some scanned files had no match, so their originals were not kept, and they changed since: they were not searched.".into());
    }
    data.notes.push("Immutable original snapshot search; does not assert the sources are still current. Matches are text windows, not structured JSON records.".into());
    Ok(data)
}

/// Store a recovery result and render its page at `offset`, as
/// `render::render` does. A result whose only record is one whole stored text
/// original (a single-line blob paged or searched shows its only line) names
/// that blob, as an automatic view's artifact does (schema 2), instead of
/// storing the original a second time for every recovery.
pub fn render(data: &Dataset, store: &Store, budget: usize, offset: usize) -> Result<View> {
    if let [record] = data.records.as_slice()
        && let Some(blob) = record.blob.as_deref()
        // A scanned file whose original was not kept is searched locally:
        // its blob is named but absent, so the result keeps its own copy.
        && blob.starts_with("blob:")
        && store.contains(blob)
        // Hashing settles that the text is the whole blob, not a prefix...
        && blob.ends_with(&digest(record.text.as_bytes()))
        // ...and reading it back as text must give exactly this record.
        && crate::sources::Parsed::Text(record.text.clone()).records(&record.source, blob.into())
            == data.records
    {
        let artifact = store.put_json(&DatasetRef::new(
            data,
            RecordsRef {
                blob: blob.into(),
                format: Format::Text,
                source: record.source.clone(),
            },
        ))?;
        return crate::render::render_stored(data, artifact, Mode::Default, budget, offset);
    }
    crate::render::render(data, store, Mode::Default, budget, offset)
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
            view.stop_early(NOTE);
        }
        (serde_json::to_vec(&view).ok()?.len() < budget).then_some(view)
    };
    // A view holds its text: lines longer than the budget cannot fit, and are
    // not rendered to find that out.
    let whole_fits =
        |data: &Dataset| data.records[0].text.len() <= budget && measure(data, None).is_some();
    if data.records.is_empty() || whole_fits(&data) {
        return render(&data, store, budget, 0);
    }
    let record = &data.records[0];
    // Where each line ends: a prefix of whole lines is a slice of the text.
    let ends: Vec<usize> = record
        .text
        .split_inclusive('\n')
        .scan(0, |end, line| {
            *end += line.len();
            Some(*end)
        })
        .collect();
    let first = record.start_line.unwrap_or(start);
    // A candidate copies its own lines only, never the whole selection.
    let take = |count: usize| Dataset {
        schema_version: data.schema_version,
        scan_complete: data.scan_complete,
        examined: data.examined,
        skipped: data.skipped.clone(),
        notes: data.notes.clone(),
        snapshots: data.snapshots.clone(),
        records: vec![Record {
            source: record.source.clone(),
            blob: record.blob.clone(),
            start_line: record.start_line,
            end_line: Some(first + count - 1),
            text: record.text[..ends[count - 1]].to_owned(),
            value: None,
            omitted_lines: record.omitted_lines,
            text_truncated: record.text_truncated,
        }],
    };
    // The largest prefix of whole lines that fits, by bisection. A prefix
    // longer than the budget cannot fit, which bounds the search from above
    // without rendering it.
    let beyond = ends.partition_point(|&end| end <= budget) + 1;
    let (mut fits, mut over) = (0, beyond.min(ends.len()));
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
        return render(&data, store, budget, 0);
    }
    let mut view = render(&take(fits), store, budget, 0)?;
    view.next_start = Some(first + fits);
    view.stop_early(NOTE);
    Ok(view)
}

/// Lines `start..=end` of a blob, read from its verified bytes.
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
    let (selected, count) = crate::line_index::slice(&text, start, end);
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
