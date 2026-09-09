use scopelet::{
    model::{Dataset, Format, Mode, Operation, Record, Source},
    pipeline, process, render, sources,
    store::Store,
};
use serde_json::{Value, json};
use std::process::Command;
use std::sync::{Arc, atomic::AtomicBool};
use std::time::Duration;
use tempfile::{TempDir, tempdir};

fn test_store() -> (TempDir, Store) {
    let dir = tempdir().expect("temporary directory");
    let store = Store::open(Some(dir.path().join("cache"))).expect("store");
    (dir, store)
}

fn cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

#[test]
fn search_all_patterns_across_lines_and_merges_overlapping_context() {
    let (dir, store) = test_store();
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, "before\nalpha\nbeta\nafter\n").expect("fixture");
    let source = Source::File {
        path: path.to_string_lossy().into_owned(),
        format: Format::Text,
    };
    let mut data = sources::load(&source, &store, cancel()).expect("load");

    pipeline::apply(
        &mut data,
        &[Operation::Search {
            patterns: vec!["alpha".into(), "beta".into()],
            all: true,
            regex: false,
            context: 1,
        }],
    )
    .expect("search");

    assert_eq!(data.records.len(), 1, "overlapping context should merge");
    let record = &data.records[0];
    assert_eq!(record.start_line, Some(1));
    assert_eq!(record.end_line, Some(4));
    assert_eq!(record.text, "before\nalpha\nbeta\nafter\n");
}

#[test]
fn regex_search_can_match_across_physical_lines() {
    let (dir, store) = test_store();
    let path = dir.path().join("notes.txt");
    let original = "prefix\nalpha\nbeta\nsuffix\n";
    std::fs::write(&path, original).expect("fixture");
    let source = Source::File {
        path: path.to_string_lossy().into_owned(),
        format: Format::Text,
    };
    let mut data = sources::load(&source, &store, cancel()).expect("load");

    pipeline::apply(
        &mut data,
        &[Operation::Search {
            patterns: vec!["alpha\\nbeta".into()],
            all: false,
            regex: true,
            context: 0,
        }],
    )
    .expect("search");

    assert_eq!(data.records.len(), 1);
    assert_eq!(data.records[0].text, original);
    assert_eq!(data.records[0].start_line, Some(1));
    assert_eq!(data.records[0].end_line, Some(4));
}

#[test]
fn json_operations_preserve_large_numbers_and_distinguish_missing_from_null() {
    let (dir, store) = test_store();
    let path = dir.path().join("records.json");
    std::fs::write(
        &path,
        r#"[{"id":9007199254740993,"group":"x","value":null},{"id":9007199254740993,"group":"x"},{"id":9007199254740994,"group":"y","value":null}]"#,
    )
    .expect("fixture");
    let source = Source::File {
        path: path.to_string_lossy().into_owned(),
        format: Format::Json,
    };

    let mut projected = sources::load(&source, &store, cancel()).expect("load");
    pipeline::apply(
        &mut projected,
        &[Operation::Project {
            pointers: vec!["/id".into(), "/group".into()],
        }],
    )
    .expect("project");
    let projected_id = projected.records[0]
        .value
        .as_ref()
        .unwrap()
        .get("/id")
        .unwrap();
    assert_eq!(projected_id.to_string(), "9007199254740993");

    let mut grouped = sources::load(&source, &store, cancel()).expect("load");
    pipeline::apply(
        &mut grouped,
        &[Operation::Group {
            pointer: "/id".into(),
        }],
    )
    .expect("group");
    let keys: Vec<String> = grouped
        .records
        .iter()
        .map(|record| {
            record
                .value
                .as_ref()
                .unwrap()
                .get("key")
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(keys, vec!["9007199254740993", "9007199254740994"]);

    let mut nulls = sources::load(&source, &store, cancel()).expect("load");
    pipeline::apply(
        &mut nulls,
        &[Operation::Filter {
            pointer: "/value".into(),
            equals: Value::Null,
        }],
    )
    .expect("filter explicit null");
    assert_eq!(nulls.records.len(), 2);

    let mut missing = sources::load(&source, &store, cancel()).expect("load");
    pipeline::apply(
        &mut missing,
        &[Operation::Filter {
            pointer: "/value".into(),
            equals: json!(1),
        }],
    )
    .expect("filter missing value");
    assert!(missing.records.is_empty());
}

#[test]
fn malformed_jsonl_does_not_leave_partial_records_for_an_aggregate() {
    let (_dir, store) = test_store();
    let mut data = Dataset::default();
    let error = sources::ingest(
        &mut data,
        &store,
        "fixture.jsonl",
        br#"{"group":"ok"}
not-json
{"group":"later"}
"#
        .to_vec(),
        None,
        Format::Jsonl,
    )
    .expect_err("malformed JSONL must fail");

    assert!(format!("{error:#}").contains("malformed JSONL at fixture.jsonl:2"));
    assert!(
        data.records.is_empty(),
        "a caller must not be able to aggregate records before the malformed line"
    );
}

#[test]
fn render_budget_pages_and_artifact_expansion() {
    let (_dir, store) = test_store();
    let data = Dataset {
        records: (0..64)
            .map(|i| Record::derived(json!({"id": i, "payload": "fixed"})))
            .collect(),
        ..Dataset::default()
    };

    let first = render::render(&data, &store, Mode::Default, 1024, 0).expect("first page");
    assert!(first.shown_records > 0);
    assert!(first.shown_records < first.total_records);
    let next = first.next_offset.expect("next page");
    assert_eq!(next, first.shown_records);
    assert!(!first.display_complete);

    let artifact_data: Dataset =
        serde_json::from_slice(&store.get(&first.artifact).expect("artifact bytes"))
            .expect("artifact dataset");
    let second =
        render::render(&artifact_data, &store, Mode::Default, 4096, next).expect("expanded page");
    assert_eq!(second.offset, next);
    assert!(second.shown_records > 0);
    assert_eq!(second.records[0].value.as_ref().unwrap()["id"], json!(next));
}

#[test]
fn ultra_mode_marks_long_text_as_abridged() {
    let (_dir, store) = test_store();
    let mut data = Dataset::default();
    data.records.push(Record {
        source: "long.txt".into(),
        blob: None,
        start_line: Some(1),
        end_line: Some(20),
        text: (1..=20)
            .map(|i| format!("line {i}: {}\n", "x".repeat(80)))
            .collect(),
        value: None,
        omitted_lines: None,
        text_truncated: false,
    });

    let view = render::render(&data, &store, Mode::Ultra, 4096, 0).expect("ultra view");
    assert!(!view.display_complete);
    assert_eq!(view.records.len(), 1);
    assert_eq!(view.records[0].omitted_lines, Some(12));
    assert!(view.records[0].text.lines().count() <= 8);
    assert!(view.notes.iter().any(|note| note.contains("Partial view")));
}

#[test]
fn stale_artifact_is_rejected_after_source_changes() {
    let (dir, store) = test_store();
    let path = dir.path().join("source.txt");
    std::fs::write(&path, "original\n").expect("fixture");
    let source = Source::File {
        path: path.to_string_lossy().into_owned(),
        format: Format::Text,
    };
    let data = sources::load(&source, &store, cancel()).expect("load");
    let view = render::render(&data, &store, Mode::Default, 4096, 0).expect("render");
    std::fs::write(&path, "changed\n").expect("mutate fixture");

    let artifact = Source::Artifact { id: view.artifact };
    let error = sources::load(&artifact, &store, cancel()).expect_err("stale artifact");
    assert!(format!("{error:#}").contains("source changed or unavailable"));
}

#[test]
fn cache_references_reject_path_traversal() {
    let (_dir, store) = test_store();
    for id in [
        "blob:../outside",
        "blob:../../../../etc/passwd",
        "../artifacts/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "unknown:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ] {
        assert!(
            store.get(id).is_err(),
            "cache reference should be rejected: {id}"
        );
    }
}

#[test]
fn failed_command_preserves_raw_stdout_and_stderr() {
    let cancel = cancel();
    let mut command = Command::new("sh");
    command.args(["-c", "printf '\\001\\002'; printf '\\003\\004' >&2; exit 7"]);
    let result =
        process::capture(&mut command, Duration::from_secs(2), 1024, cancel).expect("capture");

    assert_eq!(process::exit_code(&result), 7);
    assert_eq!(result.stdout, vec![1, 2]);
    assert_eq!(result.stderr, vec![3, 4]);
    assert!(!result.timed_out);
    assert!(!result.capped);
}

#[test]
fn timed_out_command_is_killed_and_gets_timeout_exit_code() {
    let cancel = cancel();
    let mut command = Command::new("sh");
    command.args(["-c", "sleep 2; printf late"]);
    let result =
        process::capture(&mut command, Duration::from_millis(40), 1024, cancel).expect("capture");

    assert!(result.timed_out);
    assert_eq!(process::exit_code(&result), 124);
    assert!(
        result.stdout.is_empty(),
        "timed-out child output should be bounded"
    );
}

#[test]
fn clean_tolerates_foreign_files_and_reaps_stale_temporaries() {
    let (_dir, store) = test_store();
    let blob = store.put("blob", b"kept original\n").expect("blob");
    let artifacts = store.root.join("artifacts");
    // A desktop indexer file and a temporary from a write that was killed.
    std::fs::write(artifacts.join(".DS_Store"), b"finder").expect("foreign file");
    std::fs::write(
        store
            .root
            .join("blobs")
            .join(".scopelet-write-ABCDEF123456"),
        b"partial",
    )
    .expect("temporary");

    let removed = store
        .clean(0)
        .expect("clean must not fail on foreign names");

    assert_eq!(removed, 2, "the orphan blob and the stale temporary");
    assert!(
        artifacts.join(".DS_Store").exists(),
        "files that are not cache items are left alone"
    );
    assert!(store.get(&blob).is_err(), "unreferenced blob was removed");
    assert_eq!(store.clean(0).expect("second clean"), 0);
}

#[test]
fn clean_preserves_unrecognized_temporary_names_in_both_directories() {
    let (_dir, store) = test_store();
    let names = [".tmp-user-notes", ".tmpABCDEF", ".scopelet-write-notes"];
    for kind in ["blobs", "artifacts"] {
        for name in names {
            std::fs::write(store.root.join(kind).join(name), b"foreign data").unwrap();
        }
    }
    assert_eq!(store.clean(0).unwrap(), 0);
    for kind in ["blobs", "artifacts"] {
        for name in names {
            assert_eq!(
                std::fs::read(store.root.join(kind).join(name)).unwrap(),
                b"foreign data"
            );
        }
    }
}

#[test]
fn clean_keeps_originals_of_a_surviving_artifact_next_to_foreign_files() {
    let (_dir, store) = test_store();
    let mut data = Dataset::default();
    sources::ingest(
        &mut data,
        &store,
        "f",
        b"keep me\n".to_vec(),
        None,
        Format::Text,
    )
    .expect("ingest");
    let view = render::render(&data, &store, Mode::Default, 4096, 0).expect("render");
    std::fs::write(store.root.join("artifacts").join("notes.txt"), b"note").expect("foreign file");

    assert_eq!(store.clean(7).expect("clean"), 0);
    assert!(store.get(&view.artifact).is_ok());
    assert!(store.get(data.records[0].blob.as_ref().unwrap()).is_ok());
}

#[test]
fn ultra_never_ships_an_empty_record_for_an_oversized_line() {
    let (_dir, store) = test_store();
    let mut data = Dataset::default();
    data.records.push(Record {
        source: "minified.js".into(),
        blob: None,
        start_line: Some(1),
        end_line: Some(2),
        text: format!("{}\ntail\n", "y".repeat(4096)),
        value: None,
        omitted_lines: None,
        text_truncated: false,
    });

    let view = render::render(&data, &store, Mode::Ultra, 4096, 0).expect("ultra view");

    assert_eq!(view.shown_records, 1);
    let record = &view.records[0];
    assert!(!record.text.is_empty(), "a counted record must carry text");
    assert!(record.text_truncated, "the cut inside a line is reported");
    assert_eq!(record.text.len(), 1024);
    assert_eq!(record.end_line, Some(1));
    assert_eq!(record.omitted_lines, Some(1));
    assert!(!view.display_complete);
    assert!(
        view.notes
            .iter()
            .any(|note| note.contains("Ultra cut a line"))
    );
}

#[test]
fn ultra_cuts_an_oversized_line_on_a_character_boundary() {
    let (_dir, store) = test_store();
    let mut data = Dataset::default();
    data.records.push(Record {
        source: "wide.txt".into(),
        blob: None,
        start_line: Some(1),
        end_line: Some(1),
        text: format!("{}\n", "é".repeat(1000)),
        value: None,
        omitted_lines: None,
        text_truncated: false,
    });

    let view = render::render(&data, &store, Mode::Ultra, 4096, 0).expect("ultra view");

    let text = &view.records[0].text;
    assert!(text.chars().all(|c| c == 'é'));
    assert_eq!(text.len(), 1024);
    assert_eq!(view.records[0].omitted_lines, Some(0));
}

#[test]
fn paging_a_stored_artifact_writes_no_new_cache_items() {
    let (_dir, store) = test_store();
    let mut data = Dataset::default();
    let text: String = (1..=200).map(|i| format!("{{\"id\":{i}}}\n")).collect();
    sources::ingest(
        &mut data,
        &store,
        "f",
        text.into_bytes(),
        None,
        Format::Jsonl,
    )
    .expect("ingest");
    let first = render::render(&data, &store, Mode::Default, 1024, 0).expect("first page");
    let before = std::fs::read_dir(store.root.join("artifacts"))
        .expect("artifacts")
        .count();

    let next = first.next_offset.expect("more records");
    let second = render::render_stored(&data, first.artifact.clone(), Mode::Default, 1024, next)
        .expect("second page");

    assert_eq!(second.artifact, first.artifact);
    assert_eq!(second.offset, next);
    assert_eq!(
        std::fs::read_dir(store.root.join("artifacts"))
            .expect("artifacts")
            .count(),
        before,
        "paging must not persist another copy of the dataset"
    );
}
