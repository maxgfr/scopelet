//! Schema-2 artifacts: compact-v3 automatic compression stores the original
//! once and an artifact that names it, instead of a second copy of every
//! record. Readers parse the records again from the verified blob.
use assert_cmd::Command;
use scopelet::{
    compress::{self, Version},
    model::{Dataset, Operation, Request, Source},
    sources,
    store::Store,
};
use serde_json::Value;
use std::sync::{Arc, atomic::AtomicBool};

fn artifact(text: &str) -> &str {
    text.split_whitespace()
        .find(|s| s.starts_with("artifact:"))
        .expect("artifact reference")
}

fn compress_with(dir: &std::path::Path, raw: &[u8], version: Version) -> Option<String> {
    let output = compress::automatic_lazy(raw, Some(dir.to_path_buf()), 4096, version).unwrap();
    (output.as_ref() != raw).then(|| String::from_utf8(output.into_owned()).unwrap())
}

/// Text, JSON, JSONL and a large object: every shape automatic compression
/// detects, at sizes that compress.
fn inputs() -> Vec<(&'static str, String)> {
    let rows: Vec<String> = (0..400)
        .map(|i| {
            format!(
                "{{\"id\":{i},\"status\":\"{}\",\"exact\":9007199254740993}}",
                ["ok", "failed"][i % 2]
            )
        })
        .collect();
    let object = serde_json::to_string_pretty(&serde_json::json!({
        "items": (0..300).map(|i| serde_json::json!({"name": format!("pod-{i}"), "ready": i != 7})).collect::<Vec<_>>()
    }))
    .unwrap();
    vec![
        (
            "text",
            format!("{}error: late failure\r\nend\n", "working\n".repeat(900)),
        ),
        ("json", format!("[{}]", rows.join(","))),
        ("jsonl", rows.join("\n")),
        ("object", object),
    ]
}

#[test]
fn v3_artifacts_name_the_blob_and_rehydrate_to_the_same_dataset() {
    for (name, raw) in inputs() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(Some(dir.path().into())).unwrap();
        let text = compress_with(dir.path(), raw.as_bytes(), Version::V3)
            .unwrap_or_else(|| panic!("{name} passed through"));
        let id = artifact(&text);
        let stored: Value = serde_json::from_slice(&store.get(id).unwrap()).unwrap();
        assert_eq!(stored["schema_version"], 2, "{name}");
        assert!(stored.get("records").is_none(), "{name}");
        let blob = stored["snapshots"][0]["blob"].as_str().unwrap();
        assert_eq!(stored["records_from"]["blob"], blob, "{name}");
        assert_eq!(store.get(blob).unwrap(), raw.as_bytes(), "{name}");

        // The same records an ingestion of the original produces.
        let mut expected = Dataset::default();
        let format = compress::detect(&raw);
        sources::ingest(
            &mut expected,
            &store,
            "input",
            raw.clone().into_bytes(),
            None,
            format,
        )
        .unwrap();
        let data = store.dataset(id).unwrap();
        assert_eq!(data.schema_version, 1, "{name}: records are materialized");
        assert_eq!(data.records, expected.records, "{name}");
        assert_eq!(data.snapshots[0].blob, expected.snapshots[0].blob, "{name}");

        // And the same records a v2 artifact stores, where v2 compresses.
        if let Some(v2) = compress_with(dir.path(), raw.as_bytes(), Version::V2) {
            let v2 = store.dataset(artifact(&v2)).unwrap();
            assert_eq!(data.records, v2.records, "{name}");
            assert_eq!(data.notes, v2.notes, "{name}");
        }
        // Metadata alone does not read the blob.
        assert!(store.dataset_head(id).unwrap().records.is_empty());
        assert_eq!(
            store.references(id).unwrap(),
            vec![blob.to_owned(), blob.to_owned()]
        );
    }
}

#[test]
fn a_megabyte_input_stores_one_original_and_a_small_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let raw: String = (0..25_000)
        .map(|i| format!("{i:06} worker processed an item in the queue ok\n"))
        .collect();
    assert!(raw.len() >= 1_000_000);
    let text = compress_with(dir.path(), raw.as_bytes(), Version::V3).unwrap();
    let id = artifact(&text);
    let artifact_bytes = std::fs::metadata(dir.path().join("artifacts").join(&id[9..]))
        .unwrap()
        .len();
    assert!(artifact_bytes < 1024, "{artifact_bytes} byte artifact");
    let cached: u64 = ["artifacts", "blobs"]
        .iter()
        .flat_map(|d| std::fs::read_dir(dir.path().join(d)).unwrap())
        .map(|e| e.unwrap())
        .filter(|e| e.file_name().len() == 64)
        .map(|e| e.metadata().unwrap().len())
        .sum();
    assert!(cached < raw.len() as u64 + 1024, "{cached} bytes cached");
}

#[test]
fn an_older_reader_rejects_a_schema_2_artifact_instead_of_misreading_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().into())).unwrap();
    let (_, raw) = &inputs()[0];
    let text = compress_with(dir.path(), raw.as_bytes(), Version::V3).unwrap();
    let error = serde_json::from_slice::<Dataset>(&store.get(artifact(&text)).unwrap())
        .unwrap_err()
        .to_string();
    assert!(error.contains("missing field `records`"), "{error}");
}

#[test]
fn a_missing_or_damaged_original_fails_the_rehydration() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().into())).unwrap();
    let (_, raw) = &inputs()[0];
    let text = compress_with(dir.path(), raw.as_bytes(), Version::V3).unwrap();
    let id = artifact(&text);
    let blob = store.dataset_head(id).unwrap().snapshots[0].blob.clone();
    let path = dir.path().join("blobs").join(&blob[5..]);
    std::fs::write(&path, b"tampered").unwrap();
    let error = format!("{:#}", store.dataset(id).unwrap_err());
    assert!(error.contains("integrity mismatch"), "{error}");
    std::fs::remove_file(&path).unwrap();
    assert!(store.dataset(id).is_err());
    // Metadata stays readable: the manifest still names the lost original.
    assert_eq!(store.dataset_head(id).unwrap().snapshots[0].blob, blob);
}

#[test]
fn queries_over_a_schema_2_artifact_store_self_contained_results() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().into())).unwrap();
    let (_, raw) = &inputs()[0];
    let text = compress_with(dir.path(), raw.as_bytes(), Version::V3).unwrap();
    let request = Request {
        version: 1,
        source: Source::Artifact {
            id: artifact(&text).into(),
        },
        operations: vec![Operation::Search {
            patterns: vec!["late failure".into()],
            all: false,
            regex: false,
            context: 0,
        }],
        mode: Default::default(),
        max_bytes: None,
    };
    let data =
        scopelet::query::execute(&request, &store, Arc::new(AtomicBool::new(false))).unwrap();
    assert_eq!(data.records.len(), 1);
    let id = store.put_json(&data).unwrap();
    let stored: Value = serde_json::from_slice(&store.get(&id).unwrap()).unwrap();
    assert_eq!(stored["schema_version"], 1);
    assert_eq!(store.dataset(&id).unwrap().records, data.records);
}

#[test]
fn expand_pages_and_describes_a_schema_2_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let (_, raw) = &inputs()[0];
    let text = compress_with(dir.path(), raw.as_bytes(), Version::V3).unwrap();
    let run = |args: &[&str]| -> Value {
        let output = Command::new(assert_cmd::cargo::cargo_bin!("scopelet"))
            .arg("--cache-dir")
            .arg(dir.path())
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let manifest = run(&["expand", artifact(&text), "--manifest"]);
    assert_eq!(manifest["artifact_schema_version"], 2);
    assert_eq!(manifest["total_records"], 1);
    let page = run(&["expand", artifact(&text)]);
    assert_eq!(page["records"][0]["source"], "input");
    let found = run(&["expand", artifact(&text), "--find", "late failure"]);
    assert_eq!(found["total_records"], 1);
}
