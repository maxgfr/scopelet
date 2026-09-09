use crate::model::{Dataset, Mode, Record};
use crate::store::Store;
use anyhow::{Result, ensure};
use serde::Serialize;

#[derive(Serialize)]
pub struct View {
    pub schema_version: u32,
    pub artifact: String,
    pub mode: Mode,
    pub scan_complete: bool,
    pub display_complete: bool,
    pub total_records: usize,
    pub shown_records: usize,
    pub omitted_records: usize,
    pub offset: usize,
    pub next_offset: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_record: Option<usize>,
    pub records: Vec<Record>,
    pub notes: Vec<String>,
}

pub fn render(
    data: &Dataset,
    store: &Store,
    mode: Mode,
    budget: usize,
    offset: usize,
) -> Result<View> {
    let artifact = store.put_json(data)?;
    paginate(data, artifact, mode, budget, offset)
}

/// Page a dataset already stored under `artifact`, without re-serializing and
/// re-hashing the whole dataset for every page.
pub fn render_stored(
    data: &Dataset,
    artifact: String,
    mode: Mode,
    budget: usize,
    offset: usize,
) -> Result<View> {
    paginate(data, artifact, mode, budget, offset)
}

fn paginate(
    data: &Dataset,
    artifact: String,
    mode: Mode,
    budget: usize,
    offset: usize,
) -> Result<View> {
    ensure!(
        (512..=1024 * 1024).contains(&budget),
        "internal view budget must be 512..1048576"
    );
    ensure!(
        offset <= data.records.len(),
        "offset is past the result set"
    );
    let mut view = View {
        schema_version: 1,
        artifact,
        mode,
        scan_complete: data.scan_complete,
        display_complete: data.records.is_empty(),
        total_records: data.records.len(),
        shown_records: 0,
        omitted_records: data.records.len(),
        offset,
        next_offset: (offset < data.records.len()).then_some(offset),
        blocked_record: None,
        records: Vec::new(),
        notes: Vec::new(),
    };
    if !data.notes.is_empty() || !data.skipped.is_empty() {
        view.notes
            .push("Source notes, exclusions and snapshots are in expand --manifest.".into());
    }
    let mut abridged = false;
    let mut encoded_size = serde_json::to_vec(&view)?.len();
    for record in &data.records[offset..] {
        let shortened;
        let record = if mode == Mode::Ultra && record.value.is_none() && record.text.len() > 1024 {
            let total = record.text.split_inclusive('\n').count();
            let mut text = String::new();
            let mut kept = 0;
            let mut truncated = record.text_truncated;
            for line in record.text.split_inclusive('\n').take(8) {
                if text.len() + line.len() > 1024 {
                    break;
                }
                text.push_str(line);
                kept += 1;
            }
            if kept == 0 {
                let line = record.text.split_inclusive('\n').next().unwrap();
                let cut = (0..=1024)
                    .rev()
                    .find(|&i| line.is_char_boundary(i))
                    .unwrap();
                text.push_str(&line[..cut]);
                kept = 1;
                truncated = true;
            }
            shortened = Record {
                source: record.source.clone(),
                blob: record.blob.clone(),
                start_line: record.start_line,
                end_line: Some(record.start_line.unwrap_or(1) + kept - 1),
                text,
                value: None,
                omitted_lines: Some(total - kept),
                text_truncated: truncated,
            };
            &shortened
        } else {
            record
        };
        let remaining = budget.saturating_sub(encoded_size + 257);
        let Some(size) = crate::encoding::size(record, remaining) else {
            if view.records.is_empty() {
                view.blocked_record = Some(offset);
            }
            break;
        };
        let record_size = size + 1;
        encoded_size += record_size;
        view.records.push(record.clone());
        abridged |= view.records.last().unwrap().omitted_lines.is_some();
    }
    view.shown_records = view.records.len();
    view.omitted_records = view.total_records - view.shown_records;
    view.next_offset =
        (offset + view.shown_records < view.total_records).then_some(offset + view.shown_records);
    if view.blocked_record.is_some() {
        view.next_offset = None;
        view.notes.push("Record exceeds budget. Increase max_bytes or expand --raw to a local file and select the record locally.".into());
    }
    if view.records.iter().any(|record| record.text_truncated) {
        view.notes.push(
            "Ultra cut a line longer than its limit; expand that record's blob for exact bytes."
                .into(),
        );
    }
    view.display_complete = offset == 0 && view.shown_records == view.total_records && !abridged;
    if !view.display_complete {
        view.notes.push("Partial view. Expand the artifact to page records, or its blob references for exact source bytes.".into());
    }
    ensure!(
        serde_json::to_vec(&view)?.len() < budget,
        "metadata exceeds output budget; increase max_bytes"
    );
    Ok(view)
}
