#![cfg(unix)]

use assert_cmd::Command;
use scopelet::store::Store;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;
use std::process::Output;

// PATH is scoped to each child, so fake adapters cannot race other tests.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    bin: PathBuf,
    cache: PathBuf,
    response: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        let bin = dir.path().join("bin");
        let cache = dir.path().join("cache");
        let response = dir.path().join("response.json");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&bin).unwrap();
        fs::write(root.join("source.rs"), "header\r\nfn café() {}\r\nfooter\n").unwrap();
        Self {
            _dir: dir,
            root,
            bin,
            cache,
            response,
        }
    }

    fn adapter(&self, name: &str, response: &str) {
        fs::write(&self.response, response).unwrap();
        let executable = self.bin.join(name);
        fs::write(
            &executable,
            "#!/bin/sh\n/bin/cat \"$SCOPELET_FAKE_RESPONSE\"\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn cli(&self) -> Command {
        let mut command = Command::new(assert_cmd::cargo::cargo_bin!("scopelet"));
        command
            .env("PATH", &self.bin)
            .env("SCOPELET_FAKE_RESPONSE", &self.response)
            .arg("--cache-dir")
            .arg(&self.cache);
        command
    }

    fn query(&self, source: Value) -> Output {
        self.cli()
            .args(["query", "--spec", "-"])
            .write_stdin(json!({"version": 1, "source": source}).to_string())
            .output()
            .unwrap()
    }

    fn code(&self) -> Value {
        json!({"type": "code", "path": self.root, "symbol": "café"})
    }

    fn definitions(&self, definition: Value) {
        self.adapter(
            "codeindex",
            &json!({"schemaVersion": 5, "defs": {"café": [definition]}}).to_string(),
        );
    }

    fn artifact(&self, source: Value) -> Value {
        let view = success(self.query(source));
        let store = Store::open(Some(self.cache.clone())).unwrap();
        serde_json::from_slice(&store.get(view["artifact"].as_str().unwrap()).unwrap()).unwrap()
    }
}

fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn rejected(output: Output, expected: &str) {
    assert_eq!(output.status.code(), Some(2));
    assert!(
        output.stdout.is_empty(),
        "failed source emitted partial evidence"
    );
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains(expected), "expected {expected:?}: {error}");
}

#[test]
fn codeindex_rejects_invalid_json_and_incompatible_symbols_schema() {
    let f = Fixture::new();
    for response in ["not JSON", "{\"schemaVersion\":4,\"defs\":{}}", "null"] {
        f.adapter("codeindex", response);
        rejected(
            f.query(f.code()),
            if response == "not JSON" {
                "incompatible JSON"
            } else {
                "schema"
            },
        );
    }
    for defs in [json!([]), json!({"café": "wrong"}), json!({"café": [{}]})] {
        f.adapter(
            "codeindex",
            &json!({"schemaVersion":5,"defs":defs}).to_string(),
        );
        let output = f.query(f.code());
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn codeindex_rejects_outside_and_reversed_spans() {
    let f = Fixture::new();
    for (start, end) in [(0, 1), (2, 1), (1, 4), (4, 4)] {
        f.definitions(json!({"file":"source.rs","line":start,"endLine":end}));
        rejected(f.query(f.code()), "span");
    }
}

#[test]
fn codeindex_rejects_present_but_noninteger_end_line() {
    let f = Fixture::new();
    for end in [json!("3"), json!(-1), json!(1.5), json!({})] {
        f.definitions(json!({"file":"source.rs","line":2,"endLine":end}));
        let output = f.query(f.code());
        assert_eq!(
            output.status.code(),
            Some(2),
            "malformed endLine accepted: {end}"
        );
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn codeindex_rejects_parent_absolute_and_symlink_escapes() {
    let f = Fixture::new();
    let outside = f.root.parent().unwrap().join("outside.rs");
    fs::write(&outside, "PRIVATE-OUTSIDE-EVIDENCE\n").unwrap();
    symlink(&outside, f.root.join("escape.rs")).unwrap();
    for file in [
        "../outside.rs".to_owned(),
        outside.to_str().unwrap().to_owned(),
        "escape.rs".to_owned(),
    ] {
        f.definitions(json!({"file":file,"line":1,"endLine":1}));
        rejected(f.query(f.code()), "escapes repository");
    }
}

#[test]
fn codeindex_exact_excerpt_retains_original_and_checks_freshness() {
    let f = Fixture::new();
    f.definitions(json!({"file":"source.rs","line":2,"endLine":2}));
    let view = success(f.query(f.code()));
    let id = view["artifact"].as_str().unwrap();
    let store = Store::open(Some(f.cache.clone())).unwrap();
    let data: Value = serde_json::from_slice(&store.get(id).unwrap()).unwrap();
    assert_eq!(data["scan_complete"], false);
    assert_eq!(data["records"][0]["text"], "fn café() {}\r\n");
    assert_eq!(data["records"][0]["start_line"], 2);
    assert_eq!(data["records"][0]["end_line"], 2);
    let blob = data["records"][0]["blob"].as_str().unwrap();
    let original = fs::read(f.root.join("source.rs")).unwrap();
    assert_eq!(store.get(blob).unwrap(), original);
    success(f.query(json!({"type":"artifact","id":id})));
    fs::write(f.root.join("source.rs"), "changed\n").unwrap();
    rejected(
        f.query(json!({"type":"artifact","id":id})),
        "source changed",
    );
    fs::remove_file(f.root.join("source.rs")).unwrap();
    rejected(
        f.query(json!({"type":"artifact","id":id})),
        "source changed",
    );
    let recovery = f.cli().args(["expand", blob, "--raw"]).output().unwrap();
    assert!(recovery.status.success());
    assert_eq!(recovery.stdout, original);
}

#[test]
fn webindex_rejects_malformed_missing_empty_and_http_error_text() {
    let f = Fixture::new();
    for (response, expected) in [
        ("not JSON", "incompatible JSON"),
        ("{}", "missing text"),
        ("{\"text\":null}", "missing text"),
        ("{\"text\":\" \\n\"}", "empty extracted text"),
        ("{\"text\":\"not found\",\"status\":404}", "HTTP status 404"),
    ] {
        f.adapter("webindex", response);
        rejected(
            f.query(json!({"type":"url","url":"https://example.invalid/"})),
            expected,
        );
    }
}

#[test]
fn webindex_text_is_exact_and_never_certifies_source_completeness() {
    let f = Fixture::new();
    f.adapter(
        "webindex",
        &json!({"text":"résumé\r\nline two\n","status":200,"warnings":["partial extraction"]})
            .to_string(),
    );
    let data = f.artifact(json!({"type":"url","url":"https://example.invalid/"}));
    assert_eq!(data["scan_complete"], false);
    assert_eq!(data["records"][0]["text"], "résumé\r\nline two\n");
    let store = Store::open(Some(f.cache.clone())).unwrap();
    let blob = data["records"][0]["blob"].as_str().unwrap();
    assert_eq!(store.get(blob).unwrap(), "résumé\r\nline two\n".as_bytes());
}

#[test]
fn extraction_metadata_and_text_survive_cleanup_of_unreferenced_blobs() {
    let f = Fixture::new();
    let envelope =
        json!({"text":"exact extraction","warnings":["tables omitted"],"engine":"fixture"});
    f.adapter("webindex", &envelope.to_string());
    let data = f.artifact(json!({"type":"url","url":"https://example.invalid/"}));
    let metadata = data["snapshots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|snapshot| {
            snapshot["source"]
                .as_str()
                .unwrap()
                .starts_with("webindex:metadata:")
        })
        .expect("metadata must be referenced by the artifact, not only mentioned in notes");
    let metadata_id = metadata["blob"].as_str().unwrap();
    let text_id = data["records"][0]["blob"].as_str().unwrap();
    let store = Store::open(Some(f.cache.clone())).unwrap();
    let orphan_id = store.put("blob", b"unreferenced").unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86400);
    for id in [metadata_id, text_id, &orphan_id] {
        fs::File::open(f.cache.join("blobs").join(id.split_once(':').unwrap().1))
            .unwrap()
            .set_modified(old)
            .unwrap();
    }
    assert_eq!(store.clean(1).unwrap(), 1);
    assert!(store.get(&orphan_id).is_err());
    assert_eq!(
        serde_json::from_slice::<Value>(&store.get(metadata_id).unwrap()).unwrap(),
        envelope
    );
    assert_eq!(store.get(text_id).unwrap(), b"exact extraction");
}

#[test]
fn nonzero_adapter_status_rejects_even_well_formed_json() {
    let f = Fixture::new();
    f.adapter("codeindex", "{\"schemaVersion\":5,\"defs\":{}}");
    fs::write(
        f.bin.join("codeindex"),
        "#!/bin/sh\n/bin/cat \"$SCOPELET_FAKE_RESPONSE\"\nprintf 'fixture failed' >&2\nexit 7\n",
    )
    .unwrap();
    rejected(f.query(f.code()), "codeindex failed (7): fixture failed");
}

#[test]
fn extracted_local_document_artifact_rejects_changed_original() {
    let f = Fixture::new();
    let document = f.root.join("document.pdf");
    fs::write(&document, b"original document bytes").unwrap();
    f.adapter(
        "webindex",
        &json!({"text":"extracted contents"}).to_string(),
    );
    let view = success(f.query(json!({"type":"document","path":document})));
    let id = view["artifact"].as_str().unwrap();
    success(f.query(json!({"type":"artifact","id":id})));
    fs::write(&document, b"different document bytes").unwrap();
    rejected(
        f.query(json!({"type":"artifact","id":id})),
        "source changed",
    );
}

#[test]
fn invalid_adapter_arguments_fail_before_missing_executable() {
    let f = Fixture::new();
    rejected(
        f.query(json!({"type":"url","url":"file:///private/file"})),
        "HTTP(S)",
    );
    rejected(
        f.query(json!({"type":"document","path":"--output=bad"})),
        "option",
    );
    rejected(
        f.query(json!({"type":"code","path":f.root,"relation":"callers","symbol":"--help"})),
        "option",
    );
}

#[test]
fn missing_adapters_have_actionable_errors_and_native_sources_still_work() {
    let f = Fixture::new();
    rejected(
        f.query(f.code()),
        "codeindex unavailable; run scopelet doctor",
    );
    rejected(
        f.query(json!({"type":"url","url":"https://example.invalid/"})),
        "webindex unavailable; run scopelet doctor",
    );
    success(f.query(json!({"type":"file","path":f.root.join("source.rs")})));
    let doctor = success(f.cli().arg("doctor").output().unwrap());
    for adapter in doctor["optional_adapters"].as_array().unwrap() {
        assert_eq!(adapter["available"], false);
        assert!(adapter["install"].as_str().unwrap().contains("install"));
    }
}

#[test]
fn store_rejects_reference_path_traversal_and_symlink_items() {
    let f = Fixture::new();
    let store = Store::open(Some(f.cache.clone())).unwrap();
    for id in [
        "blob:../../outside",
        "artifact:/etc/passwd",
        "other:0000000000000000000000000000000000000000000000000000000000000000",
    ] {
        assert!(store.get(id).is_err());
    }
    let id = store.put("blob", b"original").unwrap();
    let path = f.cache.join("blobs").join(id.split_once(':').unwrap().1);
    let outside = f.root.join("original");
    fs::write(&outside, b"original").unwrap();
    fs::remove_file(&path).unwrap();
    symlink(&outside, &path).unwrap();
    assert!(store.get(&id).unwrap_err().to_string().contains("symlink"));
}
