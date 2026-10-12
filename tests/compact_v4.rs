//! Compact-v4 presentation contracts: a one-line envelope with a short
//! artifact ID and no footer around the compact-v3 body, with the original
//! bytes always recoverable from the store.
use scopelet::{
    commands::Profile,
    compress::{self, Version},
    model::{Dataset, Record},
    store::Store,
};

fn compress_v4(dir: &std::path::Path, raw: &[u8], budget: usize) -> String {
    let output =
        compress::automatic_lazy(raw, Some(dir.to_path_buf()), budget, Version::V4).unwrap();
    assert!(output.len() <= budget, "{} bytes over budget", output.len());
    String::from_utf8(output.into_owned()).unwrap()
}

/// The 16 hex characters a v4 header shows.
fn short_id(text: &str) -> &str {
    text.strip_prefix("[scopelet v4 ")
        .and_then(|rest| rest.get(..16))
        .filter(|id| id.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap_or_else(|| panic!("no v4 header: {text}"))
}

fn original(dir: &std::path::Path, text: &str) -> Vec<u8> {
    let store = Store::open(Some(dir.to_path_buf())).unwrap();
    let artifact = store.resolve(short_id(text)).unwrap();
    let data: Dataset = store.dataset(&artifact).unwrap();
    store.get(&data.snapshots[0].blob).unwrap()
}

/// Lines no template folds: each carries a distinct word of letters.
fn distinct_lines(count: usize) -> String {
    (0..count)
        .map(|i: usize| {
            let word: String = i
                .to_string()
                .chars()
                .map(|c| (b'a' + (c as u8 - b'0')) as char)
                .collect();
            format!("{word} unit ready\n")
        })
        .collect()
}

fn first_line(text: &str) -> &str {
    text.split_inclusive('\n').next().unwrap()
}

#[test]
fn a_complete_view_has_one_short_header_line_and_no_footer() {
    let dir = tempfile::tempdir().unwrap();
    let raw = format!(
        "start\n{}fatal error: rare root cause\nend\n",
        "error: retry failed\nworking\n".repeat(1000)
    );
    let text = compress_v4(dir.path(), raw.as_bytes(), 4096);
    let header = first_line(&text);
    assert_eq!(
        header,
        format!("[scopelet v4 {} complete]\n", short_id(&text))
    );
    assert!(header.len() <= 45, "{} header bytes", header.len());
    assert_eq!(text.matches("[scopelet ").count(), 1, "{text}");
    assert!(text.contains("input:2002 fatal error: rare root cause\n"));
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

#[test]
fn a_partial_view_counts_its_omissions_in_the_header() {
    let dir = tempfile::tempdir().unwrap();
    let raw = distinct_lines(3000);
    let text = compress_v4(dir.path(), raw.as_bytes(), 4096);
    let header = first_line(&text);
    let id = short_id(&text);
    let rest = header
        .strip_prefix(&format!("[scopelet v4 {id} omitted="))
        .unwrap_or_else(|| panic!("{header}"));
    let (count, tail) = rest.split_once(';').unwrap();
    assert!(count.parse::<usize>().unwrap() > 0, "{header}");
    assert_eq!(tail, " gaps are not absence; expand ID --find TEXT]\n");
    assert!(header.len() <= 110, "{} header bytes", header.len());
    assert!(!text.contains("display_complete"), "{text}");
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

/// The body is compact-v3's: v4 only frames it differently.
#[test]
fn the_body_matches_compact_v3_line_for_line() {
    let dir = tempfile::tempdir().unwrap();
    let raw: String = (0..2000)
        .map(|i| {
            if i == 1200 {
                "error: disk quota exceeded\n".to_owned()
            } else {
                format!("worker {i} idle\n")
            }
        })
        .collect();
    let v4 = compress_v4(dir.path(), raw.as_bytes(), 4096);
    let v3 = String::from_utf8(
        compress::automatic_lazy(raw.as_bytes(), Some(dir.path().into()), 4096, Version::V3)
            .unwrap()
            .into_owned(),
    )
    .unwrap();
    let v3_body: Vec<_> = v3
        .lines()
        .filter(|l| !l.starts_with("[scopelet "))
        .collect();
    let v4_body: Vec<_> = v4.lines().skip(1).collect();
    // V4 spends the bytes its envelope saves on more evidence, never less.
    for line in &v3_body {
        assert!(v4_body.contains(line), "{line:?} missing from v4:\n{v4}");
    }
    assert!(
        v4.contains("input:1201 error: disk quota exceeded\n"),
        "{v4}"
    );
}

#[test]
fn an_incomplete_scan_says_so_in_the_header_only_when_false() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(Some(dir.path().into())).unwrap();
    let mut data = Dataset::default();
    data.records.push(Record {
        source: "src/lib.rs".into(),
        blob: None,
        start_line: Some(1),
        end_line: Some(1),
        text: "fn main() {}\n".into(),
        value: None,
        omitted_lines: None,
        text_truncated: false,
    });
    let complete = compress::compact_version(&data, &store, 4096, Version::V4).unwrap();
    assert!(!complete.contains("scan_complete"), "{complete}");
    assert!(
        first_line(&complete).ends_with(" complete]\n"),
        "{complete}"
    );
    data.incomplete("synthetic cap".into());
    let partial_scan = compress::compact_version(&data, &store, 4096, Version::V4).unwrap();
    let id = short_id(&partial_scan);
    assert_eq!(
        first_line(&partial_scan),
        format!("[scopelet v4 {id} scan_complete=false complete]\n")
    );
    assert!(store.resolve(id).is_ok());
}

#[test]
fn a_v4_view_is_recognized_as_already_compressed() {
    let dir = tempfile::tempdir().unwrap();
    let raw: String = (0..3000).map(|i| format!("row {i} value\n")).collect();
    let text = compress_v4(dir.path(), raw.as_bytes(), 4096);
    let padded = format!("{text}{}", "padding line\n".repeat(400));
    for version in [Version::V3, Version::V4] {
        let again =
            compress::automatic_lazy(padded.as_bytes(), Some(dir.path().into()), 4096, version)
                .unwrap();
        assert_eq!(again.as_ref(), padded.as_bytes());
    }
}

/// A complete JSON table has no positional footer in v4: the header says the
/// display is complete.
#[test]
fn a_complete_table_carries_only_the_header() {
    let dir = tempfile::tempdir().unwrap();
    let raw = (0..60)
        .map(|i| {
            serde_json::json!({"identifier": i, "status": "ok", "region": "eu-west"}).to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes();
    let text = compress_v4(dir.path(), &raw, 4096);
    assert!(first_line(&text).ends_with(" complete]\n"), "{text}");
    assert_eq!(text.lines().count(), 2, "{text}");
    assert!(text.lines().nth(1).unwrap().starts_with("table "), "{text}");
    assert_eq!(original(dir.path(), &text), raw);
}

fn anchored(
    dir: &std::path::Path,
    raw: &str,
    profile: Option<Profile>,
    version: Version,
    anchors: &[&str],
) -> String {
    let anchors: Vec<String> = anchors.iter().map(|s| s.to_string()).collect();
    let output = compress::automatic_anchored(
        "cli",
        raw.as_bytes(),
        Some(dir.to_path_buf()),
        profile,
        None,
        version,
        &anchors,
    );
    String::from_utf8(output.into_owned()).unwrap()
}

/// A line naming a term of the task is shown although nothing else ranks it:
/// the view depends on the anchors, the artifact does not. Earlier compact
/// versions ignore anchors.
#[test]
fn anchor_terms_rank_their_lines_into_the_view() {
    let dir = tempfile::tempdir().unwrap();
    let mut raw = distinct_lines(3000);
    let at = raw.match_indices('\n').nth(1499).unwrap().0 + 1;
    raw.insert_str(at, "bcde parse_header unit ready\n");
    let plain = anchored(dir.path(), &raw, None, Version::V4, &[]);
    assert!(!plain.contains("parse_header"), "{plain}");
    let focused = anchored(dir.path(), &raw, None, Version::V4, &["parse_header"]);
    assert!(
        focused.contains("input:1501 bcde parse_header unit ready\n"),
        "{focused}"
    );
    assert_eq!(short_id(&plain), short_id(&focused));
    let v3 = anchored(dir.path(), &raw, None, Version::V3, &["parse_header"]);
    assert!(!v3.contains("parse_header"), "{v3}");
}

/// A term on every line takes at most a third of the budget: diagnostics and
/// the first and last lines keep their place.
#[test]
fn an_anchor_on_every_line_cannot_flood_the_view() {
    let dir = tempfile::tempdir().unwrap();
    let mut raw: String = distinct_lines(3000)
        .lines()
        .map(|l| format!("tick {l}\n"))
        .collect();
    let at = raw.match_indices('\n').nth(2099).unwrap().0 + 1;
    raw.insert_str(at, "error: replica 7 lost quorum\n");
    let text = anchored(dir.path(), &raw, None, Version::V4, &["tick"]);
    assert!(text.len() <= 4096, "{}", text.len());
    // Shown lines may sit in range blocks: `input:a-b`, then one per line.
    assert!(text.contains("error: replica 7 lost quorum\n"), "{text}");
    assert!(text.contains("tick a unit ready\n"), "{text}");
    assert!(text.contains("tick cjjj unit ready\n"), "{text}");
    let ticks = text.lines().filter(|l| l.contains("tick")).count();
    let plain = anchored(dir.path(), &raw, None, Version::V4, &[]);
    let plain_ticks = plain.lines().filter(|l| l.contains("tick")).count();
    // Anchored lines replace ordinary ones; they do not crowd out evidence.
    assert!(ticks >= plain_ticks, "{ticks} < {plain_ticks}");
}

/// A file read ranks no line as a diagnostic, but anchors still apply.
#[test]
fn anchors_apply_to_file_reads() {
    let dir = tempfile::tempdir().unwrap();
    let mut source = String::new();
    for (i, line) in distinct_lines(900).lines().enumerate() {
        let word = line.split(' ').next().unwrap();
        source.push_str(&format!(
            "fn {word}(x: u32) -> u32 {{\n    x.{word}()\n}}\n"
        ));
        if i == 450 {
            source.push_str("fn resolve_prefix(text: &str) -> bool {\n    text.len() >= 8\n}\n");
        }
    }
    let plain = anchored(
        dir.path(),
        &source,
        Some(Profile::FileRead),
        Version::V4,
        &[],
    );
    assert!(!plain.contains("resolve_prefix"), "{plain}");
    let focused = anchored(
        dir.path(),
        &source,
        Some(Profile::FileRead),
        Version::V4,
        &["resolve_prefix"],
    );
    assert!(
        focused.contains("fn resolve_prefix(text: &str) -> bool {"),
        "{focused}"
    );
}

fn cli(dir: &std::path::Path) -> assert_cmd::Command {
    let mut command = assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("scopelet"));
    command
        .env("SCOPELET_COMPACT_VERSION", "4")
        .env("SCOPELET_CONFIG_DIR", dir.join("config"))
        .env("SCOPELET_CACHE_DIR", dir.join("cache"));
    command
}

fn hook(dir: &std::path::Path, agent: &str, event: serde_json::Value) -> serde_json::Value {
    let out = cli(dir)
        .args(["hook", agent])
        .write_stdin(event.to_string())
        .output()
        .unwrap();
    assert!(out.status.success());
    serde_json::from_slice(&out.stdout).unwrap()
}

/// Prompt terms reach the view through the private session state, and the
/// command's own pattern through its arguments; neither is journaled.
#[test]
fn anchor_terms_come_from_the_prompt_and_the_command() {
    let dir = tempfile::tempdir().unwrap();
    let mut raw = distinct_lines(3000);
    let at = raw.match_indices('\n').nth(1499).unwrap().0 + 1;
    raw.insert_str(at, "bcde parse_header unit ready\n");
    let tool = |command: &str| serde_json::json!({"hook_event_name":"PostToolUse","session_id":"a1","tool_name":"Bash","tool_input":{"command":command},"tool_response":{"stdout":raw,"stderr":"","interrupted":false,"isImage":false}});
    let view = |result: serde_json::Value| -> String {
        result["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    // A different session sees no prompt anchors yet.
    let before = view(hook(dir.path(), "claude", tool("npm run build")));
    assert!(!before.contains("parse_header"), "{before}");
    let prompt = serde_json::json!({"hook_event_name":"UserPromptSubmit","session_id":"a2","prompt":"Why is `parse_header` slow?"});
    hook(dir.path(), "claude", prompt);
    let mut event = tool("npm run build");
    event["session_id"] = serde_json::json!("a2");
    let after = view(hook(dir.path(), "claude", event));
    assert!(after.contains("bcde parse_header unit ready"), "{after}");
    // The command's search pattern anchors on its own.
    let mut search = tool("rg -n parse_header");
    search["session_id"] = serde_json::json!("a3");
    let found = view(hook(dir.path(), "claude", search));
    assert!(found.contains("bcde parse_header unit ready"), "{found}");
    let journal = std::fs::read_dir(dir.path().join("cache/events"))
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect::<String>();
    assert!(!journal.contains("parse_header"), "{journal}");
}
