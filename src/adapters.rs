use crate::model::{CodeRelation, Dataset, Format, Source};
use crate::process;
use crate::sources::ingest;
use crate::store::{MAX_INPUT, Store, read_bounded};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, atomic::AtomicBool};
use std::time::Duration;

fn external(program: &str, args: &[String], cancel: Arc<AtomicBool>) -> Result<Value> {
    let result = process::capture(
        Command::new(program).args(args),
        Duration::from_secs(90),
        MAX_INPUT,
        cancel,
    )
    .with_context(|| {
        format!("{program} unavailable; run scopelet doctor for installation instructions")
    })?;
    ensure!(
        process::exit_code(&result) == 0,
        "{program} failed ({}): {}",
        process::exit_code(&result),
        String::from_utf8_lossy(&result.stderr[..result.stderr.len().min(1024)])
    );
    ensure!(
        !result.capped && !result.drain_incomplete,
        "{program} output capture is incomplete"
    );
    serde_json::from_slice(&result.stdout)
        .with_context(|| format!("{program} returned incompatible JSON"))
}

pub fn load(source: &Source, store: &Store, cancel: Arc<AtomicBool>) -> Result<Dataset> {
    let mut data = Dataset::default();
    match source {
        Source::Code {
            path,
            symbol,
            relation,
        } => {
            let root = Path::new(path).canonicalize()?;
            ensure!(root.is_dir(), "code source must be a directory");
            let verb = match relation {
                CodeRelation::Definitions => "symbols",
                CodeRelation::Callers => "callers",
                CodeRelation::Impact => "impact",
            };
            let mut args = vec![verb.into()];
            if !matches!(relation, CodeRelation::Definitions) {
                let symbol = symbol
                    .as_ref()
                    .context("callers requires a symbol; impact requires a file path in symbol")?;
                ensure!(!symbol.starts_with('-'), "symbol must not be an option");
                args.push(symbol.clone());
            }
            args.extend([
                "--repo".into(),
                root.to_string_lossy().into_owned(),
                "--no-index-cache".into(),
            ]);
            let index = external("codeindex", &args, cancel)?;
            data.notes.push("codeindex adapter: syntactic index, not a compiler proof. Reported scope and inferred relations can be incomplete.".into());
            data.scan_complete = false;
            if matches!(relation, CodeRelation::Definitions) {
                ensure!(
                    index.get("schemaVersion").and_then(Value::as_u64) == Some(5),
                    "unsupported codeindex symbols schema; expected 5"
                );
                let defs = index
                    .get("defs")
                    .and_then(Value::as_object)
                    .context("codeindex JSON missing defs")?;
                let mut files = std::collections::BTreeMap::new();
                for (name, values) in defs {
                    if symbol.as_ref().is_some_and(|wanted| name != wanted) {
                        continue;
                    }
                    for def in values.as_array().context("invalid symbol definitions")? {
                        let file = def
                            .get("file")
                            .and_then(Value::as_str)
                            .context("definition missing file")?;
                        let abs = root.join(file).canonicalize()?;
                        ensure!(abs.starts_with(&root), "codeindex path escapes repository");
                        if !files.contains_key(file) {
                            let mut source = Dataset::default();
                            ingest(
                                &mut source,
                                store,
                                file,
                                read_bounded(&abs, MAX_INPUT)?,
                                Some(abs.to_string_lossy().into_owned()),
                                Format::Text,
                            )?;
                            files.insert(file.to_string(), source);
                        }
                        let unit = files.get(file).unwrap();
                        let original = &unit.records[0];
                        let start = def
                            .get("line")
                            .and_then(Value::as_u64)
                            .context("definition missing line")?
                            as usize;
                        let lines: Vec<&str> = original.text.split_inclusive('\n').collect();
                        let end = match def.get("endLine") {
                            Some(value) => value
                                .as_u64()
                                .context("definition endLine must be an unsigned integer")?
                                as usize,
                            None => start,
                        };
                        ensure!(
                            start >= 1 && end >= start && end <= lines.len(),
                            "codeindex span is outside the source file"
                        );
                        data.records.push(crate::model::Record {
                            source: original.source.clone(),
                            blob: original.blob.clone(),
                            start_line: Some(start),
                            end_line: Some(end),
                            text: lines[start - 1..end].concat(),
                            value: None,
                            omitted_lines: None,
                            text_truncated: false,
                        });
                    }
                }
                for source in files.into_values() {
                    data.snapshots.extend(source.snapshots);
                }
                data.examined = data.snapshots.len();
            } else {
                ingest(
                    &mut data,
                    store,
                    &format!("codeindex:{verb}"),
                    serde_json::to_vec(&index)?,
                    None,
                    Format::Json,
                )?;
                data.examined = 1;
            }
        }
        Source::Url { url } | Source::Document { path: url } => {
            let is_url = matches!(source, Source::Url { .. });
            if is_url {
                ensure!(
                    url.starts_with("https://") || url.starts_with("http://"),
                    "URL must use HTTP(S)"
                );
            }
            ensure!(!url.starts_with('-'), "source must not be an option");
            if !is_url {
                let original = Path::new(url).canonicalize()?;
                let bytes = read_bounded(&original, MAX_INPUT)?;
                let blob = store.put("blob", &bytes)?;
                data.snapshots.push(crate::model::Snapshot {
                    source: url.clone(),
                    blob,
                    bytes: bytes.len(),
                    local_path: Some(original.to_string_lossy().into_owned()),
                });
            }
            let result = external(
                "webindex",
                &[
                    if is_url { "fetch" } else { "extract" }.into(),
                    url.clone(),
                    "--json".into(),
                ],
                cancel,
            )?;
            // Extraction ladders can report partial results. Keep their notes and source envelope.
            let envelope_bytes = serde_json::to_vec(&result)?;
            let envelope = store.put("blob", &envelope_bytes)?;
            data.snapshots.push(crate::model::Snapshot {
                source: format!("webindex:metadata:{url}"),
                blob: envelope.clone(),
                bytes: envelope_bytes.len(),
                local_path: None,
            });
            let text = result
                .get("text")
                .and_then(Value::as_str)
                .context("webindex JSON missing text (check adapter version)")?;
            ensure!(
                !text.trim().is_empty(),
                "webindex returned empty extracted text"
            );
            if let Some(status) = result.get("status").and_then(Value::as_u64) {
                ensure!(status < 400, "webindex HTTP status {status}");
            }
            ingest(
                &mut data,
                store,
                url,
                text.as_bytes().to_vec(),
                None,
                Format::Text,
            )?;
            data.notes.push(format!(
                "Extracted text, not original document bytes. Extraction metadata: {envelope}."
            ));
            data.scan_complete = false;
            data.notes
                .push("Extraction may omit content; source completeness is not certified.".into());
            data.examined = 1;
        }
        _ => bail!("not an adapter source"),
    }
    Ok(data)
}

pub fn doctor(cancel: Arc<AtomicBool>) -> Value {
    let checks: Vec<Value> = [
        ("codeindex", "2.29.1", "npm install -g @maxgfr/codeindex@2.30.0"),
        ("webindex", "1.18.10", "brew install maxgfr/tap/webindex"),
    ].into_iter().map(|(name, tested, install)| {
        let result = process::capture(Command::new(name).arg("version"), Duration::from_secs(5), 4096, cancel.clone());
        match result {
            Ok(result) => json!({"name":name,"available":result.status.success(),"version":String::from_utf8_lossy(&result.stdout).trim(),"tested_version":tested,"install":install}),
            Err(_) => json!({"name":name,"available":false,"tested_version":tested,"install":install}),
        }
    }).collect();
    json!({"scopelet":env!("CARGO_PKG_VERSION"),"native_sources":["repo","file","artifact","run"],"optional_adapters":checks})
}
