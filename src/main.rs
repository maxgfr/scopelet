use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use scopelet::{
    compress, integration,
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
    /// Compact text representation; defaults to SCOPELET_COMPACT_VERSION or 3.
    #[arg(long, global = true, value_enum)]
    compact_version: Option<compress::Version>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Install automatic hooks without changing other integrations.
    Install {
        #[arg(long, value_enum)]
        agent: integration::Agent,
    },
    /// Remove only Scopelet hooks.
    Uninstall {
        #[arg(long, value_enum)]
        agent: integration::Agent,
    },
    /// Persist the automatic compression and response style preference.
    Mode {
        #[arg(value_enum)]
        mode: integration::Preference,
    },
    /// Agent lifecycle adapter; receives one JSON event on stdin.
    Hook {
        #[arg(value_enum)]
        agent: integration::Agent,
    },
    /// Adaptively compress stdin; small or unsuitable inputs remain byte-exact.
    Compress {
        #[arg(long, default_value_t = 4096)]
        max_bytes: usize,
    },
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
        /// Literal text; repeat --find for alternatives. Regex is available in a spec.
        #[arg(long, conflicts_with = "spec")]
        find: Vec<String>,
        // A spec carries its own operations: never accept flags it will ignore.
        #[arg(long, default_value_t = 3, conflicts_with = "spec")]
        context: usize,
        #[arg(long, conflicts_with = "spec")]
        count: bool,
        #[arg(long, conflicts_with = "spec")]
        filter: Option<String>,
        #[arg(long, requires = "filter", conflicts_with = "spec")]
        equals: Option<String>,
        #[arg(long, conflicts_with = "spec")]
        project: Vec<String>,
        #[arg(long, conflicts_with = "spec")]
        group: Option<String>,
        #[arg(long, value_enum, default_value = "json")]
        output: Output,
        #[arg(long, value_enum)]
        mode: Option<Mode>,
        #[arg(long)]
        max_bytes: Option<usize>,
    },
    /// Run once, preserve original stdout/stderr, and return bounded excerpts.
    Run {
        /// Adaptive stream output for automatic hooks; preserve small outputs verbatim.
        #[arg(long, conflicts_with_all = ["focus", "format", "mode", "max_bytes", "output"])]
        auto: bool,
        #[arg(long, value_enum, default_value = "json")]
        output: Output,
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
        /// Search literal text in immutable originals; repeat for alternatives.
        #[arg(long, conflicts_with_all = ["raw", "manifest", "start", "end"])]
        find: Vec<String>,
        #[arg(long, default_value_t = 3, requires = "find")]
        context: usize,
        /// Restrict an artifact search to an exact source label from its manifest.
        #[arg(long, requires = "find")]
        source: Option<String>,
        #[arg(long, conflicts_with_all = ["manifest", "start", "end", "find", "source"])]
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

#[derive(Clone, Copy, clap::ValueEnum)]
enum Output {
    Json,
    Compact,
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
    let compact_version = compress::Version::configured(cli.compact_version);
    match &cli.command {
        Cmd::Install { agent } => {
            print(&integration::install(*agent, false)?)?;
            return Ok(0);
        }
        Cmd::Uninstall { agent } => {
            print(&integration::install(*agent, true)?)?;
            return Ok(0);
        }
        Cmd::Mode { mode } => {
            integration::set_mode(*mode)?;
            print(&json!({"mode":mode}))?;
            return Ok(0);
        }
        Cmd::Hook { agent } => {
            // Hook failures must not prevent the host from executing the native call.
            let result = compact_version
                .and_then(|version| integration::hook_stdin_version(*agent, version))
                .unwrap_or_else(|_| json!({}));
            print(&result)?;
            return Ok(0);
        }
        _ => {}
    }
    let compact_version = if matches!(
        cli.command,
        Cmd::Compress { .. } | Cmd::Run { .. } | Cmd::Query { .. }
    ) {
        compact_version?
    } else {
        compress::Version::default()
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = cancel.clone();
    ctrlc::set_handler(move || signal.store(true, std::sync::atomic::Ordering::SeqCst))?;
    if matches!(cli.command, Cmd::Doctor) {
        let mut status = scopelet::adapters::doctor(cancel);
        status["integration"] = integration::doctor();
        print(&status)?;
        return Ok(0);
    }
    if matches!(cli.command, Cmd::Bench) {
        print(&scopelet::benchmark::offline()?)?;
        return Ok(0);
    }
    if let Cmd::Run {
        auto: true,
        command,
        timeout,
        ..
    } = &cli.command
    {
        ensure!(
            *timeout > 0 && *timeout <= 3600,
            "timeout must be 1..3600 seconds"
        );
        let result = process::capture(
            Command::new(&command[0]).args(&command[1..]),
            Duration::from_secs(*timeout),
            MAX_INPUT,
            cancel.clone(),
        )?;
        let code = process::exit_code(&result);
        for (bytes, stderr) in [(&result.stdout, false), (&result.stderr, true)] {
            let output =
                compress::automatic_lazy(bytes, cli.cache_dir.clone(), 4096, compact_version)
                    .unwrap_or(std::borrow::Cow::Borrowed(bytes));
            if stderr {
                std::io::stderr().write_all(&output)?;
            } else {
                std::io::stdout().write_all(&output)?;
            }
        }
        if result.capped || result.drain_incomplete || result.timed_out || result.interrupted {
            eprintln!(
                "[scopelet capture_complete=false exit_code={code}; saved output may be partial]"
            );
        }
        return Ok(code);
    }
    if let Cmd::Compress { max_bytes } = cli.command {
        ensure!(
            (1024..=1024 * 1024).contains(&max_bytes),
            "max_bytes must be 1024..1048576"
        );
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(MAX_INPUT as u64 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= MAX_INPUT, "input exceeds 32 MiB");
        let output = compress::automatic_lazy(&bytes, cli.cache_dir, max_bytes, compact_version)
            .unwrap_or(std::borrow::Cow::Borrowed(&bytes));
        std::io::stdout().write_all(&output)?;
        return Ok(if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            130
        } else {
            0
        });
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
            filter,
            equals,
            project,
            group,
            output,
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
                if let Some(pointer) = filter {
                    let equals = equals.context("--filter requires --equals with a JSON value")?;
                    operations.push(Operation::Filter {
                        pointer,
                        equals: serde_json::from_str(&equals)
                            .context("--equals must be JSON; quote string values")?,
                    });
                }
                if !project.is_empty() {
                    operations.push(Operation::Project { pointers: project });
                }
                if let Some(pointer) = group {
                    operations.push(Operation::Group { pointer });
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
            let data = scopelet::query::execute(&request, &store, cancel.clone())?;
            if matches!(output, Output::Compact) {
                let text = compress::compact_version(
                    &data,
                    &store,
                    request.max_bytes.unwrap_or(4096),
                    compact_version,
                )?;
                std::io::stdout().write_all(text.as_bytes())?;
                return Ok(if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                    130
                } else {
                    0
                });
            }
            print(&render::render(
                &data,
                &store,
                request.mode,
                request.max_bytes.unwrap_or(request.mode.budget()),
                0,
            )?)?;
        }
        Cmd::Run {
            auto: _,
            output,
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
            if matches!(output, Output::Compact) {
                let prefix =
                    format!("exit_code={code} stdout={stdout_blob} stderr={stderr_blob}\n");
                ensure!(budget >= prefix.len() + 512, "compact run budget too small");
                let text = compress::compact_version(
                    &data,
                    &store,
                    budget - prefix.len(),
                    compact_version,
                )?;
                std::io::stdout().write_all(format!("{prefix}{text}").as_bytes())?;
                return Ok(code);
            }
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
            find,
            context,
            source,
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
            if !find.is_empty() {
                let data =
                    scopelet::recovery::search(&store, &id, &find, context, source.as_deref())?;
                print(&render::render(
                    &data,
                    &store,
                    Mode::Default,
                    max_bytes,
                    offset,
                )?)?;
                return Ok(if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                    130
                } else {
                    0
                });
            }
            if id.starts_with("blob:") && (start.is_some() || end.is_some()) {
                ensure!(!manifest, "--manifest requires an artifact");
                let data = scopelet::recovery::range(
                    &store,
                    &id,
                    start.unwrap_or(1),
                    end.unwrap_or(usize::MAX),
                )?;
                print(&render::render(&data, &store, Mode::Default, max_bytes, 0)?)?;
                return Ok(if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                    130
                } else {
                    0
                });
            }
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
        Cmd::Doctor
        | Cmd::Bench
        | Cmd::Install { .. }
        | Cmd::Uninstall { .. }
        | Cmd::Mode { .. }
        | Cmd::Hook { .. }
        | Cmd::Compress { .. } => unreachable!(),
    }
    Ok(if cancel.load(std::sync::atomic::Ordering::SeqCst) {
        130
    } else {
        0
    })
}
