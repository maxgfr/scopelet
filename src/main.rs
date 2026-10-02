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
        /// Byte target (default 4096, or the profile's).
        #[arg(long)]
        max_bytes: Option<usize>,
        /// The kind of output, as for run --auto.
        #[arg(long, value_enum)]
        profile: Option<scopelet::commands::Profile>,
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
        /// How to parse --file (text by default).
        #[arg(long, value_enum, requires = "file", conflicts_with = "spec")]
        format: Option<Format>,
        /// Repository glob to include; repeatable.
        #[arg(long, conflicts_with_all = ["spec", "file"])]
        include: Vec<String>,
        /// Repository glob to exclude; repeatable.
        #[arg(long, conflicts_with_all = ["spec", "file"])]
        exclude: Vec<String>,
        /// Literal text; repeat --find for alternatives.
        #[arg(long, conflicts_with = "spec")]
        find: Vec<String>,
        /// Treat --find patterns as regular expressions.
        #[arg(long, requires = "find", conflicts_with = "spec")]
        regex: bool,
        /// Match --find patterns without regard to case.
        #[arg(long, requires = "find", conflicts_with = "spec")]
        ignore_case: bool,
        // A spec carries its own operations: never accept flags it will ignore.
        /// Lines around each match (default 3).
        #[arg(long, requires = "find", conflicts_with = "spec")]
        context: Option<usize>,
        #[arg(long, conflicts_with = "spec")]
        count: bool,
        #[arg(long, conflicts_with = "spec")]
        filter: Option<String>,
        #[arg(long, requires = "filter", conflicts_with = "spec")]
        equals: Option<String>,
        #[arg(long, conflicts_with = "spec")]
        project: Vec<String>,
        /// Drop records identical to an earlier one.
        #[arg(long, conflicts_with = "spec")]
        unique: bool,
        /// Group by a JSON pointer; groups come largest first.
        #[arg(long, conflicts_with = "spec")]
        group: Option<String>,
        /// Show only the first N records (the N largest groups); the saved
        /// artifact keeps every record.
        #[arg(long)]
        top: Option<usize>,
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
        #[arg(long, conflicts_with_all = ["focus", "format", "mode", "output"])]
        auto: bool,
        #[arg(long, value_enum, default_value = "json")]
        output: Output,
        #[arg(long, value_enum, default_value = "default")]
        mode: Mode,
        #[arg(long)]
        max_bytes: Option<usize>,
        /// With --auto: the kind of output, which sets the compact-v3 budget
        /// (unless --max-bytes is given) and, for file-read, reads the output
        /// as a file rather than as an outcome.
        #[arg(long, value_enum, requires = "auto")]
        profile: Option<scopelet::commands::Profile>,
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
        /// artifact:<sha256> or blob:<sha256>, a bare hash, a unique prefix of
        /// at least 8 hex characters, or last (the latest automatic view).
        id: String,
        /// Search literal text in immutable originals; repeat for alternatives.
        #[arg(long, conflicts_with_all = ["raw", "manifest", "start", "end"])]
        find: Vec<String>,
        /// Treat --find patterns as regular expressions.
        #[arg(long, requires = "find")]
        regex: bool,
        /// Match --find patterns without regard to case.
        #[arg(long, requires = "find")]
        ignore_case: bool,
        #[arg(long, default_value_t = 3, requires = "find")]
        context: usize,
        /// Restrict an artifact search to an exact source label from its manifest.
        #[arg(long, requires = "find")]
        source: Option<String>,
        #[arg(long, conflicts_with_all = ["manifest", "start", "end", "find", "source"])]
        raw: bool,
        #[arg(long)]
        manifest: bool,
        /// Record position to page an artifact or a search result from.
        #[arg(long, default_value_t = 0, conflicts_with_all = ["start", "end"])]
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
    /// Summarize the local event journal: compressions, bytes saved, why
    /// outputs passed through, and how often a compressed view was expanded.
    Stats {
        #[arg(long, default_value_t = 30)]
        days: u64,
    },
    /// Explicitly remove cached artifacts and originals older than this age.
    Clean {
        #[arg(long, default_value_t = 7)]
        older_days: u64,
        /// Then evict least recently used items until the cache fits, e.g.
        /// 500M or 2G; items modified within the last hour are kept.
        #[arg(long, value_parser = parse_size)]
        max_size: Option<u64>,
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

/// A byte size: digits with an optional K, M or G suffix (binary multiples).
fn parse_size(text: &str) -> Result<u64, String> {
    let (digits, unit) = match text.find(|c: char| !c.is_ascii_digit()) {
        Some(i) => text.split_at(i),
        None => (text, ""),
    };
    let shift = match unit
        .to_ascii_uppercase()
        .trim_end_matches("IB")
        .trim_end_matches('B')
    {
        "" => 0,
        "K" => 10,
        "M" => 20,
        "G" => 30,
        _ => {
            return Err(format!(
                "invalid size {text:?}; use bytes or a K, M or G suffix"
            ));
        }
    };
    digits
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(1 << shift))
        .ok_or_else(|| format!("invalid size {text:?}"))
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
/// `schema` is the stored artifact's: a schema-2 artifact names the blob its
/// records are parsed from, which the manifest says.
fn manifest_view(data: &Dataset, max_bytes: usize, schema: u32) -> Result<serde_json::Value> {
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
    if schema != 1 {
        object.insert("artifact_schema_version".into(), json!(schema));
    }
    ensure!(
        serde_json::to_vec(&value)?.len() <= max_bytes,
        "manifest metadata exceeds output budget; increase max_bytes"
    );
    Ok(value)
}

/// The first `top` records of a stored dataset. The artifact keeps them all;
/// the view counts them all, so the rest is omitted, never absent.
fn top_view(
    data: &Dataset,
    store: &Store,
    mode: Mode,
    budget: usize,
    top: usize,
) -> Result<render::View> {
    const NOTE: &str =
        "--top limited this view; the artifact holds every record (page it with expand --offset).";
    let artifact = store.put_json(data)?;
    let shown = Dataset {
        records: data.records[..top].to_vec(),
        ..data.clone()
    };
    // Leave room for the note and the larger counts patched in below.
    let mut view = render::render_stored(&shown, artifact, mode, budget - NOTE.len() - 64, 0)?;
    view.total_records = data.records.len();
    view.omitted_records = view.total_records - view.shown_records;
    view.next_offset = (view.blocked_record.is_none()).then_some(view.shown_records);
    view.stop_early(NOTE);
    Ok(view)
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
    scopelet::process::cancel_on_termination(cancel.clone())?;
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
        max_bytes,
        profile,
        ..
    } = &cli.command
    {
        ensure!(
            max_bytes.is_none_or(|b| (1024..=1024 * 1024).contains(&b)),
            "max_bytes must be 1024..1048576"
        );
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
            let output = compress::automatic_recorded(
                "run",
                bytes,
                cli.cache_dir.clone(),
                *profile,
                *max_bytes,
                compact_version,
            );
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
    if let Cmd::Stats { days } = cli.command {
        print(&scopelet::events::stats(cli.cache_dir, days))?;
        return Ok(0);
    }
    if let Cmd::Compress { max_bytes, profile } = cli.command {
        ensure!(
            max_bytes.is_none_or(|b| (1024..=1024 * 1024).contains(&b)),
            "max_bytes must be 1024..1048576"
        );
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(MAX_INPUT as u64 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= MAX_INPUT, "input exceeds 32 MiB");
        let output = compress::automatic_recorded(
            "cli",
            &bytes,
            cli.cache_dir,
            profile,
            max_bytes,
            compact_version,
        );
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
            include,
            exclude,
            find,
            regex,
            ignore_case,
            context,
            count,
            filter,
            equals,
            project,
            unique,
            group,
            top,
            output,
            mode,
            max_bytes,
        } => {
            let compact = matches!(output, Output::Compact);
            ensure!(
                !(compact && mode.is_some()),
                "--mode applies to JSON output; compact output has its own selection"
            );
            ensure!(!(compact && top.is_some()), "--top applies to JSON output");
            ensure!(top.is_none_or(|n| n > 0), "--top must be at least 1");
            // clap does not enforce `requires` next to a conflicting --repo.
            ensure!(
                format.is_none() || file.is_some(),
                "--format applies to --file; a repository is searched as text"
            );
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
                    Source::File {
                        path,
                        format: format.unwrap_or_default(),
                    }
                } else {
                    Source::Repo {
                        path: repo.unwrap_or_else(|| ".".into()),
                        include,
                        exclude,
                    }
                };
                let mut operations = Vec::new();
                if !find.is_empty() {
                    operations.push(Operation::Search {
                        patterns: find,
                        all: false,
                        regex,
                        ignore_case,
                        context: context.unwrap_or(3),
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
                if unique {
                    operations.push(Operation::Unique);
                }
                if let Some(pointer) = group {
                    // A shortcut answers "which are most common": largest first.
                    operations.push(Operation::Group {
                        pointer,
                        order: GroupOrder::Count,
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
            let data = scopelet::query::execute(&request, &store, cancel.clone())?;
            if matches!(output, Output::Compact) {
                let text = compress::compact_version(
                    &data,
                    &store,
                    request.max_bytes.unwrap_or(compress::DEFAULT_BUDGET),
                    compact_version,
                )?;
                std::io::stdout().write_all(text.as_bytes())?;
                return Ok(if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                    130
                } else {
                    0
                });
            }
            let budget = request.max_bytes.unwrap_or(request.mode.budget());
            match top {
                Some(top) if top < data.records.len() => {
                    print(&top_view(&data, &store, request.mode, budget, top)?)?
                }
                _ => print(&render::render(&data, &store, request.mode, budget, 0)?)?,
            }
        }
        Cmd::Run {
            auto: _,
            profile: _,
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
            regex,
            ignore_case,
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
            let id = store.resolve(&id)?;
            if store.contains(&id) {
                scopelet::events::expanded(Some(store.root.clone()), &id);
            }
            if !find.is_empty() {
                let data = scopelet::recovery::search(
                    &store,
                    &id,
                    &find,
                    (regex, ignore_case),
                    context,
                    source.as_deref(),
                )?;
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
            if id.starts_with("blob:") && !raw {
                ensure!(!manifest, "--manifest requires an artifact");
                // A blob is paged by lines: the lines that fit, then next_start.
                print(&scopelet::recovery::page(
                    &store,
                    &id,
                    start.unwrap_or(1),
                    end.unwrap_or(usize::MAX),
                    max_bytes,
                )?)?;
                return Ok(if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                    130
                } else {
                    0
                });
            }
            let bytes = store.get(&id)?;
            // Expansion is use: keep the item out of the next age-based cleanup.
            store.touch(&id);
            if raw {
                std::io::stdout().write_all(&bytes)?;
                return Ok(0);
            }
            if id.starts_with("artifact:") {
                let data = store.dataset(&id)?;
                if manifest {
                    let schema = scopelet::store::artifact_schema(&bytes)?;
                    print(&manifest_view(&data, max_bytes, schema)?)?;
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
            }
        }
        Cmd::Clean {
            older_days,
            max_size,
        } => {
            let report = store.clean_with(older_days, max_size)?;
            // Session markers only gate hook context: a failure to purge them
            // does not fail the cleanup, but the count is then unknown (null).
            let sessions = integration::clean_sessions(older_days).ok();
            let mut value = serde_json::to_value(report)?;
            value["sessions_removed"] = json!(sessions);
            value["cache"] = json!(store.root);
            print(&value)?
        }
        Cmd::Doctor
        | Cmd::Bench
        | Cmd::Install { .. }
        | Cmd::Uninstall { .. }
        | Cmd::Mode { .. }
        | Cmd::Hook { .. }
        | Cmd::Stats { .. }
        | Cmd::Compress { .. } => unreachable!(),
    }
    Ok(if cancel.load(std::sync::atomic::Ordering::SeqCst) {
        130
    } else {
        0
    })
}
