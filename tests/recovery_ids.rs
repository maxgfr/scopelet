//! Recovery ergonomics: references as people type them, regex and
//! case-insensitive search of originals, and blobs paged by lines.
use scopelet::{
    compress::{self, Version},
    store::Store,
};
use serde_json::Value;

fn cli(cache: &std::path::Path, args: &[&str]) -> std::process::Output {
    assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("scopelet"))
        .arg("--cache-dir")
        .arg(cache)
        .args(args)
        .output()
        .unwrap()
}

fn json(output: &std::process::Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn compressed(cache: &std::path::Path, raw: &str) -> String {
    let output =
        compress::automatic_lazy(raw.as_bytes(), Some(cache.into()), 4096, Version::V3).unwrap();
    let text = String::from_utf8(output.into_owned()).unwrap();
    text.split_whitespace()
        .find(|s| s.starts_with("artifact:"))
        .expect("compressed")
        .to_owned()
}

fn log(lines: usize) -> String {
    (0..lines)
        .map(|i| format!("line {i:05} worker processed the item\n"))
        .chain(std::iter::once(
            "thread 'main' PANICKED at src/lib.rs:4:2\n".into(),
        ))
        .collect()
}

#[test]
fn references_resolve_from_hashes_prefixes_and_last() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().into())).unwrap();
    let error = format!("{:#}", store.resolve("last").unwrap_err());
    assert!(error.contains("no automatic compression"), "{error}");

    let artifact = compressed(dir.path(), &log(500));
    let hash = &artifact["artifact:".len()..];
    assert_eq!(store.resolve(&artifact).unwrap(), artifact);
    assert_eq!(store.resolve(hash).unwrap(), artifact);
    assert_eq!(
        store.resolve(&hash[..8].to_ascii_uppercase()).unwrap(),
        artifact
    );
    assert_eq!(store.resolve(" last\n").unwrap(), artifact);

    // A full hash names one item even when a blob has the same bytes.
    let twin = store.put("blob", &store.get(&artifact).unwrap()).unwrap();
    assert_eq!(twin, format!("blob:{hash}"));
    assert_eq!(store.resolve(hash).unwrap(), artifact);
    let error = format!("{:#}", store.resolve(&hash[..8]).unwrap_err());
    assert!(error.contains("matches 2 cached items"), "{error}");

    for bad in ["abc", "zzzzzzzzzz", "artifact:short"] {
        assert!(store.resolve(bad).is_err(), "{bad}");
    }
    assert!(store.resolve("00000000").is_err());

    // The latest automatic compression replaces `last`.
    let newer = compressed(dir.path(), &log(600));
    assert_eq!(store.resolve("last").unwrap(), newer);
}

#[test]
fn expand_finds_by_prefix_with_regex_and_ignored_case() {
    let dir = tempfile::tempdir().unwrap();
    let artifact = compressed(dir.path(), &log(500));
    let prefix = &artifact["artifact:".len().."artifact:".len() + 8];
    let found = json(&cli(
        dir.path(),
        &[
            "expand",
            prefix,
            "--find",
            r"panick(ed)? at \S+",
            "--regex",
            "--ignore-case",
            "--context",
            "0",
        ],
    ));
    assert_eq!(found["total_records"], 1);
    assert_eq!(found["records"][0]["start_line"], 501);
    // Literal by default: the same pattern finds nothing.
    let literal = json(&cli(
        dir.path(),
        &["expand", "last", "--find", r"panick(ed)?"],
    ));
    assert_eq!(literal["total_records"], 0);
    // Case matters unless asked otherwise.
    let exact = json(&cli(dir.path(), &["expand", "last", "--find", "panicked"]));
    assert_eq!(exact["total_records"], 0);
    // --regex and --ignore-case belong to --find.
    assert!(
        !cli(dir.path(), &["expand", "last", "--regex"])
            .status
            .success()
    );
}

#[test]
fn a_whole_blob_is_paged_by_lines_until_it_is_complete() {
    let dir = tempfile::tempdir().unwrap();
    let raw = log(3000);
    compressed(dir.path(), &raw);
    let manifest = json(&cli(dir.path(), &["expand", "last", "--manifest"]));
    let blob = manifest["snapshots"][0]["blob"]
        .as_str()
        .unwrap()
        .to_owned();

    let (mut text, mut start, mut pages) = (String::new(), 1u64, 0);
    loop {
        let start_arg = start.to_string();
        let page = json(&cli(
            dir.path(),
            &[
                "expand",
                &blob,
                "--start",
                &start_arg,
                "--max-bytes",
                "4096",
            ],
        ));
        assert!(page.get("blocked_record").is_none(), "{page}");
        let record = &page["records"][0];
        assert_eq!(record["start_line"], start);
        text.push_str(record["text"].as_str().unwrap());
        pages += 1;
        match page["next_start"].as_u64() {
            Some(next) => {
                assert_eq!(page["display_complete"], false);
                assert_eq!(record["end_line"].as_u64().unwrap() + 1, next);
                start = next;
            }
            None => break,
        }
    }
    assert!(pages > 5, "{pages} pages");
    assert_eq!(text, raw);

    // Without --start the first page comes back the same way.
    let first = json(&cli(dir.path(), &["expand", &blob, "--max-bytes", "4096"]));
    assert_eq!(first["records"][0]["start_line"], 1);
    assert!(first["next_start"].is_u64());

    // A small blob fits whole, with no next_start.
    let small = json(&cli(dir.path(), &["expand", &blob, "--start", "2990"]));
    assert!(small.get("next_start").is_none());
    assert_eq!(small["display_complete"], true);
}

#[test]
fn offset_with_a_line_range_is_rejected_instead_of_ignored() {
    let dir = tempfile::tempdir().unwrap();
    compressed(dir.path(), &log(500));
    let output = cli(
        dir.path(),
        &["expand", "last", "--offset", "2", "--start", "1"],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
}

/// Total bytes of stored artifacts.
fn artifact_bytes(cache: &std::path::Path) -> u64 {
    std::fs::read_dir(cache.join("artifacts"))
        .unwrap()
        .map(|entry| entry.unwrap().metadata().unwrap())
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
        .sum()
}

/// Paging or searching a single-line original shows that whole line as one
/// record. Its artifact names the blob instead of storing the line again, so
/// recovering a 2 MB line twice stores kilobytes, not two more copies, and
/// the artifact still expands to the exact line.
#[test]
fn recovering_a_single_line_original_stores_no_second_copy() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().into())).unwrap();
    let line = format!(
        "{{\"payload\":\"{}\",\"key\":\"k39\"}}",
        "x".repeat(2_000_000)
    );
    let blob = store.put("blob", line.as_bytes()).unwrap();
    let before = artifact_bytes(dir.path());
    let page = json(&cli(dir.path(), &["expand", &blob]));
    let found = json(&cli(dir.path(), &["expand", &blob, "--find", "k39"]));
    let stored = artifact_bytes(dir.path()) - before;
    assert!(
        stored < 64 * 1024,
        "{stored} artifact bytes for two recoveries"
    );
    for view in [page, found] {
        let artifact = view["artifact"].as_str().unwrap();
        let data = store.dataset(artifact).unwrap();
        assert_eq!(data.records.len(), 1);
        assert_eq!(data.records[0].text, line);
        assert_eq!(data.records[0].blob.as_deref(), Some(blob.as_str()));
    }
}
