//! Deterministic regression gate over the shared offline content fixtures.
//!
//! The fixtures mirror `bench/content.py` byte for byte (checked against the
//! recorded SHA-256 values) so the CI gate and the published comparison
//! measure the same inputs. Thresholds are minimum byte reductions of the
//! default compact presentation; facts must stay visible in the view and the
//! original bytes must remain recoverable from the store.
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
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/bench/results/content-2026-09-11.json"
    );
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
    for (name, fixture) in fixtures() {
        let hash = scopelet::store::digest(&fixture.bytes);
        assert_eq!(
            recorded[name], hash,
            "{name} fixture drifted from bench/content.py"
        );
    }
}

fn check(name: &str, fixture: &Fixture, cache: &std::path::Path) -> Result<(), String> {
    let output = compress::automatic_lazy(
        &fixture.bytes,
        Some(cache.to_path_buf()),
        compress::DEFAULT_BUDGET,
        Version::default(),
    )
    .map_err(|e| format!("{name}: {e:#}"))?;
    let Some(minimum) = fixture.minimum_reduction else {
        if output.as_ref() != fixture.bytes {
            return Err(format!("{name} must pass through unchanged"));
        }
        return Ok(());
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
        if !String::from_utf8_lossy(&original).contains(fact.as_str()) {
            return Err(format!("{name}: {fact:?} lost from the original"));
        }
    }
    Ok(())
}

#[test]
fn default_compression_meets_reduction_visibility_and_recovery_gates() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let failures: Vec<String> = fixtures()
        .iter()
        .filter_map(|(name, fixture)| check(name, fixture, &cache).err())
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
