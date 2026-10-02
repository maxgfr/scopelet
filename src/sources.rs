use crate::model::{Dataset, Format, Record, Snapshot, Source};
use crate::store::{MAX_INPUT, Store, read_bounded};
use anyhow::{Context, Result, ensure};
use ignore::{WalkBuilder, overrides::OverrideBuilder};
use serde_json::Value;
use std::path::Path;
use std::sync::{Arc, atomic::AtomicBool};

pub fn load(source: &Source, store: &Store, cancel: Arc<AtomicBool>) -> Result<Dataset> {
    load_search(source, store, cancel, None)
}

pub(crate) fn load_search(
    source: &Source,
    store: &Store,
    cancel: Arc<AtomicBool>,
    search: Option<&crate::search::Search>,
) -> Result<Dataset> {
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
            // Enumerate in path order from metadata alone; the caps apply in
            // that order, so the scanned scope never depends on timing.
            let mut candidates = Vec::new();
            let (mut files, mut bytes) = (0usize, 0u64);
            let mut interrupted = false;
            for entry in walk.build() {
                if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                    interrupted = true;
                    break;
                }
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => {
                        candidates.push(Candidate::Entry);
                        continue;
                    }
                };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    continue;
                }
                if files >= 20000 || bytes >= 128 * 1024 * 1024 {
                    candidates.push(Candidate::Capped);
                    break;
                }
                files += 1;
                // Oversize files are not read and never counted toward the cap.
                let size = entry
                    .metadata()
                    .map(|m| m.len())
                    .ok()
                    .filter(|&s| s <= MAX_INPUT as u64);
                bytes += size.unwrap_or(0);
                candidates.push(Candidate::File {
                    name: entry
                        .path()
                        .strip_prefix(&root)?
                        .to_string_lossy()
                        .into_owned(),
                    path: entry.into_path(),
                    readable: size.is_some(),
                });
            }
            let outcomes = scan(&candidates, store, search, &cancel)?;
            for (candidate, outcome) in candidates.into_iter().zip(outcomes) {
                match (candidate, outcome) {
                    (Candidate::Entry, _) => {
                        data.skip("unreadable_entry");
                        data.incomplete("some entries could not be read".into());
                    }
                    (Candidate::Capped, _) => data.incomplete(
                        "scan stopped at 20000 files or 128 MiB; narrow the scope".into(),
                    ),
                    (Candidate::File { .. }, None) => {
                        interrupted = true;
                        break;
                    }
                    (Candidate::File { .. }, Some(outcome)) => {
                        data.examined += 1;
                        match outcome {
                            Outcome::Unreadable => {
                                data.skip("unreadable_or_oversize");
                                data.incomplete("some files unreadable or over 32 MiB".into());
                            }
                            Outcome::Binary => data.skip("binary_or_non_utf8"),
                            Outcome::Text { snapshot, records } => {
                                data.snapshots.push(snapshot);
                                data.records.extend(records);
                            }
                        }
                    }
                }
            }
            if interrupted {
                data.incomplete("scan interrupted".into());
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
            let id = &store.resolve(id)?;
            if id.starts_with("artifact:") {
                data = store.dataset(id)?;
                // Records must still describe their sources. A scanned file
                // that contributed no record only widens the scope; its
                // change is reported, not fatal.
                let used: std::collections::BTreeSet<&str> = data
                    .records
                    .iter()
                    .filter_map(|r| r.blob.as_deref())
                    .collect();
                // Only records read straight from local files (a repository
                // scan) are independent of the other files. A computed record
                // (a count, a group) or an extraction depends on every source.
                let local: std::collections::BTreeSet<&str> = data
                    .snapshots
                    .iter()
                    .filter(|s| s.local_path.is_some())
                    .map(|s| s.blob.as_str())
                    .collect();
                let strict = !data
                    .records
                    .iter()
                    .all(|r| r.blob.as_deref().is_some_and(|b| local.contains(b)));
                let mut changed = 0;
                for s in &data.snapshots {
                    if let Some(path) = &s.local_path {
                        let unchanged = read_bounded(Path::new(path), MAX_INPUT)
                            .is_ok_and(|raw| s.blob.ends_with(&crate::store::digest(&raw)));
                        if unchanged {
                            continue;
                        }
                        ensure!(
                            !strict && !used.contains(s.blob.as_str()),
                            "source changed or unavailable: {path}; re-run the source query, or expand the original blob explicitly"
                        );
                        changed += 1;
                    }
                }
                for _ in 0..changed {
                    data.skip("changed_since_scan");
                }
                if changed > 0 {
                    data.notes.push("Some scanned files without a matching record changed since the scan; re-run the source query to include them.".into());
                }
            } else {
                ingest(&mut data, store, id, store.get(id)?, None, Format::Text)?;
                data.examined = 1;
            }
        }
        Source::Code { .. } | Source::Url { .. } | Source::Document { .. } => {
            return crate::adapters::load(source, store, cancel);
        }
    }
    Ok(data)
}

/// One entry of a repository walk, in path order.
enum Candidate {
    /// The walker could not read an entry.
    Entry,
    /// The file or byte cap was reached here; nothing after it is scanned.
    Capped,
    File {
        name: String,
        path: std::path::PathBuf,
        /// Its metadata was read and it is at most 32 MiB.
        readable: bool,
    },
}

enum Outcome {
    Unreadable,
    Binary,
    Text {
        snapshot: Snapshot,
        records: Vec<Record>,
    },
}

/// Worker threads for a repository scan: `SCOPELET_WORKERS` (1 to 8), else
/// the available parallelism up to 8.
fn workers() -> usize {
    std::env::var("SCOPELET_WORKERS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, usize::from))
        .clamp(1, 8)
}

/// Read, verify, hash and search the files on worker threads; outcomes come
/// back in candidate order, `None` for a file skipped after cancellation.
/// Every snapshot names its content hash, but only a file that produced a
/// record has its original stored.
fn scan(
    candidates: &[Candidate],
    store: &Store,
    search: Option<&crate::search::Search>,
    cancel: &AtomicBool,
) -> Result<Vec<Option<Outcome>>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let next = AtomicUsize::new(0);
    let work = || -> Result<Vec<(usize, Outcome)>> {
        let mut done = Vec::new();
        loop {
            let i = next.fetch_add(1, Ordering::Relaxed);
            let Some(candidate) = candidates.get(i) else {
                return Ok(done);
            };
            if cancel.load(Ordering::SeqCst) {
                return Ok(done);
            }
            if let Candidate::File {
                name,
                path,
                readable,
            } = candidate
            {
                done.push((i, file_outcome(name, path, *readable, store, search)?));
            }
        }
    };
    let threads = workers().min(candidates.len().max(1));
    let batches: Vec<Result<Vec<(usize, Outcome)>>> = if threads == 1 {
        vec![work()]
    } else {
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..threads).map(|_| scope.spawn(work)).collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("scan worker panicked")))
                })
                .collect()
        })
    };
    let mut outcomes: Vec<Option<Outcome>> = candidates.iter().map(|_| None).collect();
    for batch in batches {
        for (i, outcome) in batch? {
            outcomes[i] = Some(outcome);
        }
    }
    Ok(outcomes)
}

fn file_outcome(
    name: &str,
    path: &Path,
    readable: bool,
    store: &Store,
    search: Option<&crate::search::Search>,
) -> Result<Outcome> {
    let raw = match readable.then(|| read_bounded(path, MAX_INPUT)) {
        Some(Ok(raw)) => raw,
        _ => return Ok(Outcome::Unreadable),
    };
    if raw.contains(&0) {
        return Ok(Outcome::Binary);
    }
    let Ok(text) = String::from_utf8(raw) else {
        return Ok(Outcome::Binary);
    };
    let blob = format!("blob:{}", crate::store::digest(text.as_bytes()));
    let snapshot = Snapshot {
        source: name.into(),
        blob: blob.clone(),
        bytes: text.len(),
        local_path: Some(path.to_string_lossy().into_owned()),
    };
    // The original is kept only when the file contributes a record.
    if search.is_none_or(|s| s.matches(&text)) {
        store.put("blob", text.as_bytes())?;
    } else {
        return Ok(Outcome::Text {
            snapshot,
            records: Vec::new(),
        });
    }
    let records = Parsed::Text(text).records(name, blob);
    let records = match search {
        Some(search) => search.apply(records),
        None => records,
    };
    Ok(Outcome::Text { snapshot, records })
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
    let parsed = Parsed::parse(text, format, source)?;
    data.records.extend(parsed.records(source, blob));
    Ok(())
}

pub(crate) enum Parsed {
    Text(String),
    Json(Value),
    Jsonl(Vec<(usize, Value)>),
}

impl Parsed {
    pub(crate) fn detected(text: String) -> Self {
        // A JSON document can only start with one of these bytes after
        // whitespace; anything else is text without paying for a parse.
        let json = text
            .bytes()
            .find(|b| !matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
            .is_some_and(|b| {
                matches!(
                    b,
                    b'{' | b'[' | b'"' | b'-' | b'0'..=b'9' | b't' | b'f' | b'n'
                )
            });
        if !json {
            return Self::Text(text);
        }
        if let Ok(value) = serde_json::from_str(&text) {
            return Self::Json(value);
        }
        if crate::clean::line_count(&text) > 1 {
            let rows: Result<Vec<_>, _> = text
                .lines()
                .enumerate()
                .map(|(i, line)| serde_json::from_str(line).map(|v| (i + 1, v)))
                .collect();
            if let Ok(rows) = rows {
                return Self::Jsonl(rows);
            }
        }
        Self::Text(text)
    }

    /// The format that parses the same text back into these records.
    pub(crate) fn format(&self) -> Format {
        match self {
            Self::Text(_) => Format::Text,
            Self::Json(_) => Format::Json,
            Self::Jsonl(_) => Format::Jsonl,
        }
    }

    pub(crate) fn parse(text: String, format: Format, source: &str) -> Result<Self> {
        Ok(match format {
            Format::Text => Self::Text(text),
            Format::Json => Self::Json(
                serde_json::from_str(&text)
                    .context("invalid JSON; use jsonl for newline-delimited records")?,
            ),
            Format::Jsonl => Self::Jsonl(
                text.lines()
                    .enumerate()
                    .filter(|(_, line)| !line.trim().is_empty())
                    .map(|(i, line)| {
                        serde_json::from_str(line)
                            .map(|v| (i + 1, v))
                            .with_context(|| {
                                format!(
                                    "malformed JSONL at {source}:{}; no partial aggregate emitted",
                                    i + 1
                                )
                            })
                    })
                    .collect::<Result<_>>()?,
            ),
        })
    }

    pub(crate) fn records(self, source: &str, blob: String) -> Vec<Record> {
        let base = Record {
            source: source.into(),
            blob: Some(blob),
            start_line: None,
            end_line: None,
            text: String::new(),
            value: None,
            omitted_lines: None,
            text_truncated: false,
        };
        match self {
            Self::Text(text) => vec![Record {
                end_line: Some(crate::clean::line_count(&text)),
                start_line: Some(1),
                text,
                ..base
            }],
            Self::Json(value) => {
                let values = match value {
                    Value::Array(values) => values,
                    value => vec![value],
                };
                values
                    .into_iter()
                    .enumerate()
                    .map(|(i, value)| Record {
                        source: format!("{source}#record={i}"),
                        value: Some(value),
                        ..base.clone()
                    })
                    .collect()
            }
            Self::Jsonl(rows) => rows
                .into_iter()
                .map(|(line, value)| Record {
                    start_line: Some(line),
                    end_line: Some(line),
                    value: Some(value),
                    ..base.clone()
                })
                .collect(),
        }
    }
}
