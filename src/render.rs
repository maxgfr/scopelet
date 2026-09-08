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
    ensure!(
        (512..=1024 * 1024).contains(&budget),
        "internal view budget must be 512..1048576"
    );
    ensure!(
        offset <= data.records.len(),
        "offset is past the result set"
    );
    let artifact = store.put("artifact", &serde_json::to_vec(data)?)?;
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
        let mut record = record.clone();
        if mode == Mode::Ultra && record.value.is_none() && record.text.len() > 1024 {
            let lines: Vec<&str> = record.text.split_inclusive('\n').collect();
            let total = lines.len();
            let mut text = String::new();
            let mut kept = 0;
            for line in lines.into_iter().take(8) {
                if text.len() + line.len() > 1024 {
                    break;
                }
                text.push_str(line);
                kept += 1;
            }
            record.omitted_lines = Some(total - kept);
            record.text = text;
            record.end_line = (kept > 0).then(|| record.start_line.unwrap_or(1) + kept - 1);
        }
        let record_size = serde_json::to_vec(&record)?.len() + 1;
        // Serialize each record once; reserve final counts and recovery instructions.
        if encoded_size + record_size + 256 > budget {
            if view.records.is_empty() {
                view.blocked_record = Some(offset);
            }
            break;
        }
        encoded_size += record_size;
        view.records.push(record);
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
