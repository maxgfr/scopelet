//! Deterministic regression gate over the shared offline content fixtures.
//!
//! The synthetic fixtures mirror `bench/content.py` byte for byte (checked
//! against the recorded SHA-256 values) so the CI gate and the published
//! comparison measure the same inputs. The real tool outputs live in
//! `bench/fixtures/*.txt`, described by `bench/fixtures/manifest.json`; both
//! sides read the same files. Thresholds are minimum byte reductions of the
//! default compact presentation; facts must stay visible in the view and the
//! original bytes must remain recoverable from the store. Pending facts are
//! what a planned change should make visible: they are reported, not enforced.
use scopelet::{
    compress::{self, Version},
    model::Dataset,
    store::Store,
};

use std::collections::BTreeMap;

fn noise() -> String {
    (0..1000)
        .map(|i| format!("progress {i:04}: {}\n", "unchanged ".repeat(12)))
        .collect()
}

fn rows() -> Vec<String> {
    (0..1000)
        .map(|i| {
            let (status, message) = if i == 643 {
                ("failed", "error: audit-7139 amount=47".to_owned())
            } else {
                ("passed", "stable ".repeat(12))
            };
            format!(
                "{{\"id\":{i},\"status\":\"{status}\",\"message\":\"{message}\",\"exact\":900719925474099312345,\"nullable\":null}}"
            )
        })
        .collect()
}

struct Fixture {
    bytes: Vec<u8>,
    facts: Vec<String>,
    /// Minimum byte reduction of the compact view, in percent; None = passthrough.
    minimum_reduction: Option<f64>,
    /// Facts are expected inside the compact view (otherwise only after recovery).
    facts_visible: bool,
    /// Facts a planned change should make visible: reported, never enforced.
    pending: Vec<String>,
}

/// Real tool outputs described by `bench/fixtures/manifest.json`.
fn file_fixtures() -> BTreeMap<String, Fixture> {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/bench/fixtures");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(format!("{root}/manifest.json")).unwrap()).unwrap();
    let strings = |value: &serde_json::Value| -> Vec<String> {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_owned())
            .collect()
    };
    manifest
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, spec)| {
            let fixture = Fixture {
                bytes: std::fs::read(format!("{root}/{name}.txt"))
                    .unwrap_or_else(|e| panic!("{name}: {e}")),
                facts: strings(&spec["facts"]),
                minimum_reduction: spec["min_reduction"].as_f64(),
                facts_visible: spec["visible"].as_bool().unwrap(),
                pending: strings(&spec["pending_facts"]),
            };
            (name.clone(), fixture)
        })
        .collect()
}

#[test]
fn every_fixture_file_is_described_by_the_manifest() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/bench/fixtures");
    let described = file_fixtures();
    for entry in std::fs::read_dir(root).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        if let Some(stem) = name.strip_suffix(".txt") {
            assert!(described.contains_key(stem), "{name} has no manifest entry");
        }
    }
    for (name, fixture) in &described {
        assert!(
            !fixture.facts.is_empty() || !fixture.pending.is_empty(),
            "{name} checks nothing"
        );
        assert!(
            !fixtures().contains_key(name.as_str()),
            "{name} shadows a synthetic fixture"
        );
    }
}

fn fixtures() -> BTreeMap<&'static str, Fixture> {
    let noise = noise();
    let rows = rows();
    let receipt = format!(
        "receipt audit-7139 amount=47 EUR{}\n",
        " padding".repeat(20)
    );
    let mut fixtures = BTreeMap::new();
    let mut add = |name, bytes: Vec<u8>, facts: &[&str], minimum, visible| {
        fixtures.insert(
            name,
            Fixture {
                bytes,
                facts: facts.iter().map(|s| s.to_string()).collect(),
                minimum_reduction: minimum,
                facts_visible: visible,
                pending: Vec::new(),
            },
        );
    };
    add(
        "small",
        b"No error: operation completed.\r\n".to_vec(),
        &["No error: operation completed."],
        None,
        true,
    );
    add(
        "diagnostics",
        format!("{noise}error: audit-7139 amount=47\nwarning: do NOT disable validation\n")
            .into_bytes(),
        &[
            "error: audit-7139 amount=47",
            "warning: do NOT disable validation",
        ],
        Some(96.0),
        true,
    );
    add(
        "repeated_errors",
        format!(
            "start\n{}fatal error: rare root cause\nend\n",
            "error: retry failed\nworking\n".repeat(1000)
        )
        .into_bytes(),
        &["fatal error: rare root cause"],
        Some(97.0),
        true,
    );
    add(
        "json",
        format!("[{}]", rows.join(",")).into_bytes(),
        &["error: audit-7139 amount=47", "900719925474099312345"],
        Some(97.0),
        true,
    );
    add(
        "jsonl",
        rows.join("\n").into_bytes(),
        &["error: audit-7139 amount=47", "900719925474099312345"],
        Some(97.0),
        true,
    );
    let half = noise.len() / 2;
    add(
        "hidden_receipt",
        format!("{}{receipt}{}", &noise[..half], &noise[half..]).into_bytes(),
        &["receipt audit-7139 amount=47 EUR"],
        Some(96.0),
        true,
    );
    add(
        "giant_unicode_line",
        "é".repeat(6000).into_bytes(),
        &["é".repeat(6000).as_str()],
        Some(85.0),
        false,
    );
    add(
        "crlf",
        format!(
            "{}error: original CRLF survives\r\n",
            noise.replace('\n', "\r\n")
        )
        .into_bytes(),
        &["error: original CRLF survives"],
        Some(96.0),
        true,
    );
    fixtures
}

fn recorded_hashes() -> BTreeMap<String, String> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/bench/results/content.json");
    let report: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    report["cases"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, case)| (name.clone(), case["sha256"].as_str().unwrap().to_owned()))
        .collect()
}

#[test]
fn fixtures_match_the_published_benchmark_bytes() {
    let recorded = recorded_hashes();
    // Only the synthetic fixtures are recorded: file fixtures are their own bytes.
    for (name, fixture) in fixtures() {
        let hash = scopelet::store::digest(&fixture.bytes);
        assert_eq!(
            recorded.get(name),
            Some(&hash),
            "{name} fixture drifted from bench/content.py"
        );
    }
}

/// A fact written in range-block notation (each line after a newline
/// indented by one space) names the source lines without that indent.
fn source_form(fact: &str) -> String {
    fact.replace("\n ", "\n")
}

/// Pending facts missing from the view, or an error when a gate fails.
fn check(name: &str, fixture: &Fixture, cache: &std::path::Path) -> Result<Vec<String>, String> {
    let output = compress::automatic_lazy(
        &fixture.bytes,
        Some(cache.to_path_buf()),
        compress::DEFAULT_BUDGET,
        Version::default(),
    )
    .map_err(|e| format!("{name}: {e:#}"))?;
    let shown = String::from_utf8_lossy(&output);
    let pending = fixture
        .pending
        .iter()
        .filter(|fact| !shown.contains(fact.as_str()))
        .map(|fact| format!("{name}: pending {fact:?}"))
        .collect();
    let Some(minimum) = fixture.minimum_reduction else {
        if output.as_ref() != fixture.bytes {
            return Err(format!("{name} must pass through unchanged"));
        }
        return Ok(pending);
    };
    if output.len() > compress::DEFAULT_BUDGET {
        return Err(format!(
            "{name}: {} bytes exceed the {} byte budget (passthrough)",
            output.len(),
            compress::DEFAULT_BUDGET
        ));
    }
    let reduction = 100.0 * (1.0 - output.len() as f64 / fixture.bytes.len() as f64);
    if reduction < minimum {
        return Err(format!(
            "{name}: {reduction:.1}% reduction is below the {minimum}% gate ({} bytes)",
            output.len()
        ));
    }
    let text = std::str::from_utf8(&output).map_err(|e| format!("{name}: {e}"))?;
    if !text.starts_with("[scopelet compact-v") {
        return Err(format!("{name}: missing compact header"));
    }
    if fixture.facts_visible {
        for fact in &fixture.facts {
            if !text.contains(fact) {
                return Err(format!("{name}: fact {fact:?} not visible in:\n{text}"));
            }
        }
    } else if !text.contains("text_truncated") {
        return Err(format!(
            "{name}: oversized line not marked text_truncated:\n{text}"
        ));
    }
    let artifact = text
        .split_whitespace()
        .find(|s| s.starts_with("artifact:"))
        .ok_or_else(|| format!("{name}: no artifact reference"))?;
    let store = Store::open(Some(cache.to_path_buf())).unwrap();
    let data: Dataset = serde_json::from_slice(&store.get(artifact).unwrap()).unwrap();
    if data.snapshots.len() != 1 {
        return Err(format!("{name}: expected one snapshot"));
    }
    let original = store.get(&data.snapshots[0].blob).unwrap();
    if original != fixture.bytes {
        return Err(format!("{name}: original bytes do not round-trip"));
    }
    for fact in &fixture.facts {
        if !String::from_utf8_lossy(&original).contains(&source_form(fact)) {
            return Err(format!("{name}: {fact:?} lost from the original"));
        }
    }
    Ok(pending)
}

#[test]
fn default_compression_meets_reduction_visibility_and_recovery_gates() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let synthetic = fixtures();
    let files = file_fixtures();
    let all = synthetic
        .iter()
        .map(|(name, fixture)| (*name, fixture))
        .chain(files.iter().map(|(name, fixture)| (name.as_str(), fixture)));
    let (mut failures, mut pending) = (Vec::new(), Vec::new());
    for (name, fixture) in all {
        match check(name, fixture, &cache) {
            Ok(missing) => pending.extend(missing),
            Err(error) => failures.push(error),
        }
    }
    // Reported so a run shows what the planned changes still owe.
    for line in &pending {
        eprintln!("{line}");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
