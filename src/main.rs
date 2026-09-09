use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use scopelet::{
    model::*,
    pipeline, process, render, sources,
    store::{MAX_INPUT, Store, read_bounded},
};
use serde_json::json;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, atomic::AtomicBool};
use std::time::Duration;

#[derive(Parser)]
#[command(
    version,
    about = "Compute before you send. Local, recoverable evidence for agents."
)]
struct Cli {
    #[arg(long, global = true)]
    cache_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Compose search, filtering and aggregation. Request schema: skills/scopelet/references/queries.md
    Query {
        #[arg(long)]
        spec: Option<String>,
        /// Quick text search: --repo . --find symbol --context 5
        #[arg(long, conflicts_with = "spec")]
        repo: Option<String>,
        #[arg(long, conflicts_with_all = ["spec", "repo"])]
        file: Option<String>,
        #[arg(long, value_enum, default_value = "text", conflicts_with = "spec")]
        format: Format,
        #[arg(long, conflicts_with = "spec")]
        find: Vec<String>,
        // A spec carries its own operations: never accept flags it will ignore.
        #[arg(long, default_value_t = 3, conflicts_with = "spec")]
        context: usize,
        #[arg(long, conflicts_with = "spec")]
        count: bool,
        #[arg(long, value_enum)]
        mode: Option<Mode>,
        #[arg(long)]
        max_bytes: Option<usize>,
    },
    /// Run once, preserve original stdout/stderr, and return bounded excerpts.
    Run {
        #[arg(long, value_enum, default_value = "default")]
        mode: Mode,
        #[arg(long)]
        max_bytes: Option<usize>,
        #[arg(long, default_value_t = 120)]
        timeout: u64,
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
        #[arg(long)]
        focus: Option<String>,
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Recover an immutable snapshot or page a saved result.
    Expand {
        id: String,
        #[arg(long, conflicts_with_all = ["manifest", "start", "end"])]
        raw: bool,
        #[arg(long)]
        manifest: bool,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long)]
        start: Option<usize>,
        #[arg(long)]
        end: Option<usize>,
        #[arg(long, default_value_t = 16384)]
        max_bytes: usize,
    },
    /// Show native capabilities and optional external adapters.
    Doctor,
    /// Deterministic offline correctness and byte-size check (no model calls).
    Bench,
    /// Explicitly remove cached artifacts and originals older than this age.
    Clean {
        #[arg(long, default_value_t = 7)]
        older_days: u64,
    },
}

fn main() {
    match execute(Cli::parse()) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{}", json!({"error":format!("{error:#}")}));
            std::process::exit(2);
        }
    }
}

fn print(value: &impl serde::Serialize) -> Result<()> {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    serde_json::to_writer(&mut lock, value)?;
    writeln!(lock)?;
    Ok(())
}

/// Dataset metadata without records, kept inside the same byte budget as a view.
/// A wide scan can hold thousands of snapshots, so the list is paged by budget.
fn manifest_view(data: &Dataset, max_bytes: usize) -> Result<serde_json::Value> {
    const NOTE: &str = "Manifest truncated: snapshots listed are the first of total_snapshots. Raise --max-bytes, or expand --raw for the whole artifact.";
    let mut meta = Dataset {
        schema_version: data.schema_version,
        scan_complete: data.scan_complete,
        examined: data.examined,
        skipped: data.skipped.clone(),
        notes: data.notes.clone(),
        snapshots: Vec::new(),
        records: Vec::new(),
    };
    // Reserve the truncation note and the counts appended below.
    let mut size = serde_json::to_vec(&meta)?.len() + NOTE.len() + 128;
    for snapshot in &data.snapshots {
        let entry = serde_json::to_vec(snapshot)?.len() + 1;
        if size + entry > max_bytes {
            break;
        }
        size += entry;
        meta.snapshots.push(snapshot.clone());
    }
    if meta.snapshots.len() < data.snapshots.len() {
        meta.notes.push(NOTE.into());
    }
    let shown = meta.snapshots.len();
    let mut value = serde_json::to_value(&meta)?;
    let object = value.as_object_mut().unwrap();
    object.remove("records");
    object.insert("total_records".into(), json!(data.records.len()));
    object.insert("total_snapshots".into(), json!(data.snapshots.len()));
    object.insert("shown_snapshots".into(), json!(shown));
    ensure!(
        serde_json::to_vec(&value)?.len() <= max_bytes,
        "manifest metadata exceeds output budget; increase max_bytes"
    );
    Ok(value)
}

fn execute(cli: Cli) -> Result<i32> {
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = cancel.clone();
    ctrlc::set_handler(move || signal.store(true, std::sync::atomic::Ordering::SeqCst))?;
    if matches!(cli.command, Cmd::Doctor) {
        print(&scopelet::adapters::doctor(cancel))?;
        return Ok(0);
    }
    if matches!(cli.command, Cmd::Bench) {
        print(&scopelet::benchmark::offline()?)?;
        return Ok(0);
    }
    let store = Store::open(cli.cache_dir)?;
    match cli.command {
        Cmd::Query {
            spec,
            repo,
            file,
            format,
            find,
            context,
            count,
            mode,
            max_bytes,
        } => {
            let mut request = if let Some(spec) = spec {
                let bytes = if spec == "-" {
                    let mut bytes = Vec::new();
                    std::io::stdin()
                        .take(1024 * 1024 + 1)
                        .read_to_end(&mut bytes)?;
                    ensure!(bytes.len() <= 1024 * 1024, "request over 1 MiB");
                    bytes
                } else {
                    read_bounded(std::path::Path::new(&spec), 1024 * 1024)?
                };
                serde_json::from_slice::<Request>(&bytes).context("invalid request JSON")?
            } else {
                let source = if let Some(path) = file {
                    Source::File { path, format }
                } else {
                    Source::Repo {
                        path: repo.unwrap_or_else(|| ".".into()),
                        include: Vec::new(),
                        exclude: Vec::new(),
                    }
                };
                let mut operations = Vec::new();
                if !find.is_empty() {
                    operations.push(Operation::Search {
                        patterns: find,
                        all: false,
                        regex: false,
                        context,
                    });
                }
                if count {
                    operations.push(Operation::Count);
                }
                Request {
                    version: 1,
                    source,
                    operations,
                    mode: Mode::Default,
                    max_bytes: None,
                }
            };
            if let Some(mode) = mode {
                request.mode = mode;
            }
            if max_bytes.is_some() {
                request.max_bytes = max_bytes;
            }
            pipeline::validate(&request)?;
            let mut data = sources::load(&request.source, &store, cancel.clone())?;
            pipeline::apply(&mut data, &request.operations)?;
            print(&render::render(
                &data,
                &store,
                request.mode,
                request.max_bytes.unwrap_or(request.mode.budget()),
                0,
            )?)?;
        }
        Cmd::Run {
            mode,
            max_bytes,
            timeout,
            format,
            focus,
            command,
        } => {
            ensure!(
                timeout > 0 && timeout <= 3600,
                "timeout must be 1..3600 seconds"
            );
            let budget = max_bytes.unwrap_or(mode.budget());
            ensure!(
                (1024..=1024 * 1024).contains(&budget),
                "max_bytes must be 1024..1048576"
            );
            ensure!(
                focus.as_ref().is_none_or(|q| !q.trim().is_empty()),
                "focus must not be empty"
            );
            let result = process::capture(
                Command::new(&command[0]).args(&command[1..]),
                Duration::from_secs(timeout),
                MAX_INPUT,
                cancel.clone(),
            )?;
            let code = process::exit_code(&result);
            let mut data = Dataset::default();
            data.notes.push(format!("command exit_code={code}; stdin closed; stdout and stderr captured independently, interleaving is not reconstructed"));
            if result.capped || result.drain_incomplete || result.timed_out || result.interrupted {
                data.incomplete("command interrupted, timed out or output exceeded 32 MiB per stream; saved bytes are partial".into());
            }
            // Preserve both streams even if parsing one fails.
            let stdout_blob = store.put("blob", &result.stdout)?;
            let stderr_blob = store.put("blob", &result.stderr)?;
            let parsed = (|| -> Result<()> {
                sources::ingest(
                    &mut data,
                    &store,
                    "stdout",
                    result.stdout.clone(),
                    None,
                    format,
                )?;
                sources::ingest(
                    &mut data,
                    &store,
                    "stderr",
                    result.stderr.clone(),
                    None,
                    Format::Text,
                )?;
                Ok(())
            })();
            if let Err(error) = parsed {
                print(
                    &json!({"exit_code":code,"capture_complete":!result.capped && !result.drain_incomplete && !result.timed_out && !result.interrupted,"parse_error":format!("{error:#}"),"stdout":stdout_blob,"stderr":stderr_blob}),
                )?;
                return Ok(code);
            }
            let mut chunks = Vec::new();
            for mut record in std::mem::take(&mut data.records) {
                if record.value.is_some() {
                    chunks.push(record);
                    continue;
                }
                let text = std::mem::take(&mut record.text);
                // Adapt block size so newline-heavy captures cannot create millions of records.
                let block_lines = text.split_inclusive('\n').count().div_ceil(10_000).max(20);
                let mut lines = text.split_inclusive('\n').peekable();
                let mut start = 1;
                while lines.peek().is_some() {
                    let mut text = String::new();
                    let mut count = 0;
                    for line in lines.by_ref().take(block_lines) {
                        text.push_str(line);
                        count += 1;
                    }
                    chunks.push(Record {
                        text,
                        start_line: Some(start),
                        end_line: Some(start + count - 1),
                        ..record.clone()
                    });
                    start += count;
                }
            }
            data.records = chunks;
            if let Some(query) = focus {
                pipeline::apply(&mut data, &[Operation::Rank { query }])?;
            } else if code != 0 {
                // Failed command: last stderr/stdout blocks first. Everything stays recoverable.
                data.records.reverse();
                // JSON records carry no line numbers, so do not promise them here.
                data.notes.push("Failed command: blocks displayed from the end of stderr, then stdout; each record keeps its source label, and line numbers where it has them.".into());
            }
            data.examined = 2;
            let envelope_bytes = serde_json::to_vec(
                &json!({"exit_code":code,"stdout":stdout_blob,"stderr":stderr_blob,"result":null}),
            )?
            .len();
            let view = render::render(&data, &store, mode, budget - envelope_bytes, 0)?;
            print(
                &json!({"exit_code":code,"stdout":stdout_blob,"stderr":stderr_blob,"result":view}),
            )?;
            return Ok(code);
        }
        Cmd::Expand {
            id,
            raw,
            manifest,
            offset,
            start,
            end,
            max_bytes,
        } => {
            ensure!(
                (1024..=1024 * 1024).contains(&max_bytes),
                "max_bytes must be 1024..1048576"
            );
            let bytes = store.get(&id)?;
            // Expansion is use: keep the item out of the next age-based cleanup.
            store.touch(&id)?;
            if raw {
                std::io::stdout().write_all(&bytes)?;
                return Ok(0);
            }
            if id.starts_with("artifact:") {
                let data: Dataset = serde_json::from_slice(&bytes)?;
                if manifest {
                    print(&manifest_view(&data, max_bytes)?)?;
                } else {
                    ensure!(
                        start.is_none() && end.is_none(),
                        "line ranges require a blob reference"
                    );
                    // The dataset is already stored under this id; do not write it again.
                    print(&render::render_stored(
                        &data,
                        id,
                        Mode::Default,
                        max_bytes,
                        offset,
                    )?)?;
                }
            } else {
                ensure!(!manifest, "--manifest requires an artifact");
                let mut data = Dataset::default();
                sources::ingest(&mut data, &store, &id, bytes, None, Format::Text)?;
                if start.is_some() || end.is_some() {
                    pipeline::apply(
                        &mut data,
                        &[Operation::Read {
                            start: start.unwrap_or(1),
                            end: end.unwrap_or(usize::MAX),
                        }],
                    )?;
                }
                data.notes.push(
                    "Immutable original snapshot; does not assert the source is still current."
                        .into(),
                );
                print(&render::render(&data, &store, Mode::Default, max_bytes, 0)?)?;
            }
        }
        Cmd::Clean { older_days } => {
            print(&json!({"removed":store.clean(older_days)?,"cache":store.root}))?
        }
        Cmd::Doctor | Cmd::Bench => unreachable!(),
    }
    Ok(if cancel.load(std::sync::atomic::Ordering::SeqCst) {
        130
    } else {
        0
    })
}
