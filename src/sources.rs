use crate::model::{Dataset, Format, Record, Snapshot, Source};
use crate::store::{MAX_INPUT, Store, read_bounded};
use anyhow::{Context, Result, ensure};
use ignore::{WalkBuilder, overrides::OverrideBuilder};
use serde_json::Value;
use std::path::Path;
use std::sync::{Arc, atomic::AtomicBool};

pub fn load(source: &Source, store: &Store, cancel: Arc<AtomicBool>) -> Result<Dataset> {
    let mut data = Dataset::default();
    match source {
        Source::Repo {
            path,
            include,
            exclude,
        } => {
            let root = Path::new(path).canonicalize()?;
            ensure!(root.is_dir(), "repo source must be a directory");
            let mut overrides = OverrideBuilder::new(&root);
            for glob in include {
                ensure!(!glob.starts_with('!'), "include globs cannot start with !");
                overrides.add(glob)?;
            }
            for glob in exclude {
                ensure!(!glob.starts_with('!'), "exclude globs cannot start with !");
                overrides.add(&format!("!{glob}"))?;
            }
            let mut walk = WalkBuilder::new(&root);
            walk.overrides(overrides.build()?)
                .require_git(false)
                .follow_links(false)
                .sort_by_file_path(|a, b| a.cmp(b));
            data.notes.push("Scope excludes hidden, ignored and binary files; symlinks are not followed. Counts describe this scope, not every file on disk.".into());
            let mut bytes = 0;
            for entry in walk.build() {
                if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                    data.incomplete("scan interrupted".into());
                    break;
                }
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => {
                        data.skip("unreadable_entry");
                        data.incomplete("some entries could not be read".into());
                        continue;
                    }
                };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    continue;
                }
                if data.examined >= 20000 || bytes >= 128 * 1024 * 1024 {
                    data.incomplete(
                        "scan stopped at 20000 files or 128 MiB; narrow the scope".into(),
                    );
                    break;
                }
                data.examined += 1;
                let raw = match read_bounded(entry.path(), MAX_INPUT) {
                    Ok(raw) => raw,
                    Err(_) => {
                        data.skip("unreadable_or_oversize");
                        data.incomplete("some files unreadable or over 32 MiB".into());
                        continue;
                    }
                };
                bytes += raw.len();
                if raw.contains(&0) || std::str::from_utf8(&raw).is_err() {
                    data.skip("binary_or_non_utf8");
                    continue;
                }
                let name = entry
                    .path()
                    .strip_prefix(&root)?
                    .to_string_lossy()
                    .into_owned();
                ingest(
                    &mut data,
                    store,
                    &name,
                    raw,
                    Some(entry.path().to_string_lossy().into_owned()),
                    Format::Text,
                )?;
            }
        }
        Source::File { path, format } => {
            let path = Path::new(path).canonicalize()?;
            let name = path.to_string_lossy().into_owned();
            ingest(
                &mut data,
                store,
                &name,
                read_bounded(&path, MAX_INPUT)?,
                Some(name.clone()),
                *format,
            )?;
            data.examined = 1;
        }
        Source::Artifact { id } => {
            let raw = store.get(id)?;
            if id.starts_with("artifact:") {
                data = serde_json::from_slice(&raw).context("invalid dataset artifact")?;
                ensure!(data.schema_version == 1, "unsupported artifact schema");
                for s in &data.snapshots {
                    if let Some(path) = &s.local_path {
                        let unchanged = read_bounded(Path::new(path), MAX_INPUT)
                            .is_ok_and(|raw| s.blob.ends_with(&crate::store::digest(&raw)));
                        ensure!(
                            unchanged,
                            "source changed or unavailable: {path}; re-run the source query, or expand the original blob explicitly"
                        );
                    }
                }
            } else {
                ingest(&mut data, store, id, raw, None, Format::Text)?;
                data.examined = 1;
            }
        }
        Source::Code { .. } | Source::Url { .. } | Source::Document { .. } => {
            return crate::adapters::load(source, store, cancel);
        }
    }
    Ok(data)
}

pub fn ingest(
    data: &mut Dataset,
    store: &Store,
    source: &str,
    raw: Vec<u8>,
    local_path: Option<String>,
    format: Format,
) -> Result<()> {
    let blob = store.put("blob", &raw)?;
    data.snapshots.push(Snapshot {
        source: source.into(),
        blob: blob.clone(),
        bytes: raw.len(),
        local_path,
    });
    let text = String::from_utf8(raw)
        .context("text and JSON sources must be UTF-8; original bytes are saved in cache")?;
    let base = Record {
        source: source.into(),
        blob: Some(blob),
        start_line: None,
        end_line: None,
        text: String::new(),
        value: None,
        omitted_lines: None,
    };
    let mut records = Vec::new();
    match format {
        Format::Text => {
            let end_line = text.lines().count();
            records.push(Record {
                text,
                start_line: Some(1),
                end_line: Some(end_line),
                ..base
            });
        }
        Format::Json => {
            let value: Value = serde_json::from_str(&text)
                .context("invalid JSON; use jsonl for newline-delimited records")?;
            let values = match value {
                Value::Array(values) => values,
                value => vec![value],
            };
            for (i, value) in values.into_iter().enumerate() {
                records.push(Record {
                    source: format!("{source}#record={i}"),
                    value: Some(value),
                    ..base.clone()
                });
            }
        }
        Format::Jsonl => {
            for (i, line) in text.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                let value: Value = serde_json::from_str(line).with_context(|| {
                    format!(
                        "malformed JSONL at {source}:{}; no partial aggregate emitted",
                        i + 1
                    )
                })?;
                records.push(Record {
                    start_line: Some(i + 1),
                    end_line: Some(i + 1),
                    value: Some(value),
                    ..base.clone()
                });
            }
        }
    }
    data.records.extend(records);
    Ok(())
}
