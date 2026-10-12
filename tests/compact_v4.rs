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

fn test_view(dir: &std::path::Path, raw: &str, version: Version) -> String {
    let output = compress::automatic_profile(
        raw.as_bytes(),
        Some(dir.to_path_buf()),
        Some(Profile::Tests),
        None,
        version,
    )
    .unwrap();
    String::from_utf8(output.into_owned()).unwrap()
}

/// The line right below the header, when it is the action line.
fn action_line(text: &str) -> Option<&str> {
    text.lines().nth(1).filter(|l| l.starts_with("failed="))
}

fn passes(count: usize) -> String {
    distinct_lines(count)
        .lines()
        .map(|l| format!("test {} ... ok\n", l.replace(' ', "_")))
        .collect()
}

/// Each recognized runner's failures are named, with the location to open
/// first, right below the header.
#[test]
fn failures_of_each_runner_are_named_below_the_header() {
    let dir = tempfile::tempdir().unwrap();
    let cargo = format!(
        "running 600 tests\n{}failures:\n\n---- net::tests::parses_headers stdout ----\nthread 'net::tests::parses_headers' (4242) panicked at src/net.rs:88:5:\nassertion failed\n\n---- store::tests::evicts stdout ----\nthread 'store::tests::evicts' panicked at src/store.rs:12:9:\nleft != right\n\nfailures:\n    net::tests::parses_headers\n    store::tests::evicts\n\ntest result: FAILED. 598 passed; 2 failed\n",
        passes(600)
    );
    let pytest = format!(
        "{}=================================== FAILURES ===================================\n______________________________ test_login ______________________________\n\n    def test_login():\n>       assert login() == 1\n\napp/auth.py:40: in login\n    raise KeyError('user')\nE   KeyError: 'user'\n\napp/auth.py:41: KeyError\n=========================== short test summary info ============================\nFAILED tests/test_auth.py::test_login - KeyError: 'user'\n========= 1 failed, 600 passed in 3.2s =========\n",
        passes(600)
    );
    let jest = format!(
        "{}FAIL src/cart.test.ts\n  ● cart › totals › applies the discount\n\n    expect(received).toBe(expected)\n\n      at Object.<anonymous> (node_modules/expect/build/index.js:10:3)\n      at Object.<anonymous> (src/cart.test.ts:17:22)\n\nTests:       1 failed, 600 passed\n",
        passes(600)
    );
    let go = format!(
        "{}=== RUN   TestParse\n    parse_test.go:31: want 2, got 3\n--- FAIL: TestParse (0.00s)\nFAIL\nFAIL\texample.com/p\t0.1s\n",
        passes(600)
    );
    for (raw, expected) in [
        (
            &cargo,
            "failed=2: net::tests::parses_headers (src/net.rs:88), store::tests::evicts (src/store.rs:12)",
        ),
        (
            &pytest,
            "failed=1: tests/test_auth.py::test_login (app/auth.py:41)",
        ),
        (
            &jest,
            "failed=1: cart › totals › applies the discount (src/cart.test.ts:17)",
        ),
        (&go, "failed=1: TestParse (parse_test.go:31)"),
    ] {
        let text = test_view(dir.path(), raw, Version::V4);
        assert_eq!(action_line(&text), Some(expected), "{text}");
        assert!(text.len() <= 4096, "{}", text.len());
        assert_eq!(original(dir.path(), &text), raw.as_bytes());
    }
}

/// Five failures are named and the rest counted; output no runner format
/// matches gets no line rather than a guess, and only v4 test views get one.
#[test]
fn failures_beyond_five_are_counted_and_unknown_formats_get_no_line() {
    let dir = tempfile::tempdir().unwrap();
    let many: String = (0..8)
        .map(|i| {
            format!(
                "=== RUN   TestCase{i}\n    case_test.go:{}: boom\n--- FAIL: TestCase{i} (0.00s)\n",
                10 + i
            )
        })
        .collect();
    let raw = format!("{}{many}FAIL\n", passes(600));
    let text = test_view(dir.path(), &raw, Version::V4);
    let line = action_line(&text).unwrap();
    assert!(
        line.starts_with("failed=8: TestCase0 (case_test.go:10), "),
        "{line}"
    );
    assert!(line.ends_with("TestCase4 (case_test.go:14), +3"), "{line}");

    let unknown = format!("{}error: something odd happened\n", passes(600));
    let text = test_view(dir.path(), &unknown, Version::V4);
    assert_eq!(action_line(&text), None, "{text}");

    let v3 = test_view(dir.path(), &raw, Version::V3);
    assert!(!v3.contains("failed=8"), "{v3}");
    let search = compress::automatic_profile(
        raw.as_bytes(),
        Some(dir.path().into()),
        Some(Profile::Search),
        None,
        Version::V4,
    )
    .unwrap();
    assert!(!String::from_utf8_lossy(&search).contains("failed=8"));
}

/// `grep -n` output: `path:N:text` lines, each with a distinct word so no
/// template folds them, preceded by a distinct padding listing.
fn grep_output(files: &[(&str, usize)]) -> String {
    let words: Vec<String> = distinct_lines(2000)
        .lines()
        .map(|l| l.split(' ').next().unwrap().to_owned())
        .collect();
    let mut out = String::new();
    let mut w = 0;
    for (path, count) in files {
        for k in 0..*count {
            out.push_str(&format!(
                "{path}:{}:    let {} = 1;\n",
                10 + k * 7,
                words[w]
            ));
            w += 1;
        }
    }
    out
}

/// Consecutive matches of one file are shown under one `file=` header with
/// their own `N:text`. A search that fits is shown whole; one that does not
/// shows every file's first five matches (first, last, anchored, earliest)
/// before any file's others, and a file's first block counts what it hides.
#[test]
fn search_results_are_grouped_by_file_with_five_matches_each() {
    let dir = tempfile::tempdir().unwrap();
    let small = grep_output(&[("src/alpha.rs", 100), ("src/beta.rs", 40)]);
    let text = anchored(dir.path(), &small, Some(Profile::Search), Version::V4, &[]);
    assert!(first_line(&text).ends_with(" complete]\n"), "{text}");
    assert!(
        text.contains("input:1-100 file=src/alpha.rs\n 10:    let a = 1;\n"),
        "{text}"
    );
    assert!(text.contains(" 703:    let jj = 1;\n"), "{text}");
    assert!(
        text.contains("input:101-140 file=src/beta.rs\n 10:    let baa = 1;\n"),
        "{text}"
    );
    assert!(!text.contains("more="), "{text}");
    assert_eq!(original(dir.path(), &text), small.as_bytes());

    let names: Vec<String> = (0..30).map(|f| format!("src/m{}.rs", letters(f))).collect();
    let files: Vec<(&str, usize)> = names.iter().map(|n| (n.as_str(), 40)).collect();
    let raw = grep_output(&files);
    let words: Vec<String> = distinct_lines(2000)
        .lines()
        .map(|l| l.split(' ').next().unwrap().to_owned())
        .collect();
    // Match k of file f declares words[40 f + k] on line 10 + 7k.
    let line = |f: usize, k: usize| format!(" {}:    let {} = 1;\n", 10 + k * 7, words[40 * f + k]);
    let view = |anchors: &[&str]| {
        anchored(
            dir.path(),
            &raw,
            Some(Profile::Search),
            Version::V4,
            anchors,
        )
    };
    let text = view(&[]);
    assert!(text.len() <= 8192, "{}", text.len());
    assert!(!first_line(&text).ends_with(" complete]\n"), "{text}");
    for (f, name) in names.iter().enumerate() {
        assert!(text.contains(&format!("file={name}")), "{name}: {text}");
        for k in [0, 1, 2, 39] {
            let shown = text.contains(&line(f, k))
                || text.contains(&format!("{name}:{}", &line(f, k)[1..]));
            assert!(shown, "{name} match {k}: {text}");
        }
    }
    assert!(text.contains(" more="), "{text}");
    let middle = format!("let {} = 1;", words[40 * 15 + 20]);
    assert!(!text.contains(&middle), "{text}");
    let focused = view(&[&middle]);
    assert!(focused.contains(&middle), "{focused}");
    assert_eq!(short_id(&focused), short_id(&text));
    assert_eq!(original(dir.path(), &text), raw.as_bytes());

    // Compact-v3 keeps its line labels.
    let v3 = anchored(dir.path(), &raw, Some(Profile::Search), Version::V3, &[]);
    assert!(!v3.contains("file="), "{v3}");
}

/// Context lines (`path-N-text`) join their file's block; a context line
/// whose run holds no shown match is not shown either.
#[test]
fn search_context_lines_stay_with_their_matches() {
    let dir = tempfile::tempdir().unwrap();
    let words: Vec<String> = distinct_lines(3000)
        .lines()
        .map(|l| l.split(' ').next().unwrap().to_owned())
        .collect();
    let mut raw = String::new();
    for f in 0..200 {
        let path = format!("src/{}.rs", words[f]);
        raw.push_str(&format!(
            "{path}-9-    // {} before\n{path}:10:    call_{}();\n{path}-11-    // {} after\n--\n",
            words[1000 + f],
            words[2000 + f],
            words[1500 + f]
        ));
    }
    let text = anchored(dir.path(), &raw, Some(Profile::Search), Version::V4, &[]);
    assert!(
        text.contains("input:1-3 file=src/a.rs\n 9-    // baaa before\n 10:    call_caaa();\n 11-    // bfaa after\n"),
        "{text}"
    );
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

/// One file of a unified diff: header, one hunk, and its body lines.
fn diff_file(path: &str, body: &[String]) -> String {
    let mut out = format!(
        "diff --git a/{path} b/{path}\nindex 1a2b3c4..5d6e7f8 100644\n--- a/{path}\n+++ b/{path}\n@@ -1,{0} +1,{0} @@\n",
        body.len()
    );
    for line in body {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// A lockfile or minified file's diff becomes one counted line: its changes
/// lose their protection and the budget goes to the real files.
#[test]
fn lockfile_and_generated_diffs_become_one_counted_line() {
    let dir = tempfile::tempdir().unwrap();
    let words: Vec<String> = distinct_lines(400)
        .lines()
        .map(|l| l.split(' ').next().unwrap().to_owned())
        .collect();
    let lock: Vec<String> = (0..150)
        .flat_map(|i| {
            [
                format!("-version = \"0.{i}.0\" # {}", words[i]),
                format!("+version = \"0.{i}.1\" # {}", words[i + 200]),
            ]
        })
        .collect();
    let raw = [
        diff_file(
            "src/auth.rs",
            &[
                "     fn is_expired(&self, now: u64) -> bool {".into(),
                "-        self.expires_at > now".into(),
                "+        self.expires_at >= now".into(),
                "     }".into(),
            ],
        ),
        diff_file("Cargo.lock", &lock),
        diff_file(
            "web/dist/app.min.js",
            &["-var a=1;".repeat(40), "+var a=2;".repeat(40)],
        ),
        diff_file(
            "src/billing.rs",
            &[
                "     let gross = self.total();".into(),
                "-        gross".into(),
                "+        gross - self.discount".into(),
            ],
        ),
    ]
    .concat();
    let text = anchored(dir.path(), &raw, Some(Profile::Git), Version::V4, &[]);
    assert!(
        text.contains("input:10-314 file=Cargo.lock changed=+150/-150 omitted\n"),
        "{text}"
    );
    assert!(
        text.contains("input:315-321 file=web/dist/app.min.js changed=+1/-1 omitted\n"),
        "{text}"
    );
    assert!(!text.contains("version = \"0."), "{text}");
    assert!(!text.contains("var a="), "{text}");
    assert!(text.contains("+        self.expires_at >= now"), "{text}");
    assert!(text.contains("+        gross - self.discount"), "{text}");
    assert!(text.starts_with("[scopelet v4 "), "{text}");
    assert!(
        text.contains(" omitted="),
        "the hidden lines are counted: {text}"
    );
    assert_eq!(original(dir.path(), &text), raw.as_bytes());

    let v3 = anchored(dir.path(), &raw, Some(Profile::Git), Version::V3, &[]);
    assert!(!v3.contains("changed=+"), "{v3}");
}

/// When the real files do not all fit, the densest changes come first: a
/// file of nothing but changes between two files whose changes sit among
/// context is no longer the one left out.
#[test]
fn lockfile_free_diffs_spend_the_budget_on_dense_files_first() {
    let dir = tempfile::tempdir().unwrap();
    let words: Vec<String> = distinct_lines(5000)
        .lines()
        .map(|l| l.split(' ').next().unwrap().to_owned())
        .collect();
    let sparse = |offset: usize| -> Vec<String> {
        (0..60)
            .flat_map(|i| {
                [
                    format!("     let {} = {i};", words[offset + 3 * i]),
                    format!("     keep_{}();", words[offset + 3 * i + 1]),
                    format!("     note_{}();", words[offset + 3 * i + 2]),
                    format!("+    sparse_{}();", words[offset + i]),
                ]
            })
            .collect()
    };
    let dense: Vec<String> = (0..100)
        .flat_map(|i| {
            [
                format!("-    call_{}();", words[3000 + i]),
                format!("+    call_{}();", words[4000 + i]),
            ]
        })
        .collect();
    let raw = [
        diff_file("src/a.rs", &sparse(0)),
        diff_file("src/dense.rs", &dense),
        diff_file("src/c.rs", &sparse(1000)),
    ]
    .concat();
    let dense_shown = |text: &str| {
        (0..100)
            .filter(|i| text.contains(&format!("+    call_{}();", words[4000 + i])))
            .count()
    };
    let text = anchored(dir.path(), &raw, Some(Profile::Git), Version::V4, &[]);
    assert_eq!(dense_shown(&text), 100, "{text}");
    assert!(text.contains("diff --git a/src/a.rs"), "{text}");
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
    let v3 = anchored(dir.path(), &raw, Some(Profile::Git), Version::V3, &[]);
    assert!(dense_shown(&v3) < 100, "{v3}");
}

fn letters(i: usize) -> String {
    i.to_string()
        .bytes()
        .map(|b| (b'a' + (b - b'0')) as char)
        .collect()
}

/// A directory of more than eight entries in a `find` listing is summarized
/// by count and extension, with its first and last entries; a small one is
/// listed whole.
#[test]
fn listing_folds_large_directories_and_keeps_small_ones() {
    let dir = tempfile::tempdir().unwrap();
    let mut raw = String::from(".\n./README.md\n./src\n./src/lib\n");
    for i in 0..3 {
        raw.push_str(&format!("./src/lib/{}.rs\n", letters(i)));
    }
    raw.push_str("./src/components\n");
    for i in 0..400 {
        raw.push_str(&format!("./src/components/Widget{}.tsx\n", letters(i)));
    }
    raw.push_str("./src/components/index.ts\n./src/components/types.ts\n");
    let text = anchored(dir.path(), &raw, Some(Profile::Search), Version::V4, &[]);
    assert!(text.len() < raw.len() / 3, "{text}");
    assert!(
        text.contains("./src/components/ files=402 ext=tsx:400,ts:2\n"),
        "{text}"
    );
    assert!(text.contains("./src/components/Widgeta.tsx\n"), "{text}");
    assert!(text.contains("./src/components/types.ts\n"), "{text}");
    assert!(!text.contains("Widgetbc.tsx"), "{text}");
    for i in 0..3 {
        assert!(
            text.contains(&format!("./src/lib/{}.rs\n", letters(i))),
            "{text}"
        );
    }
    assert!(text.contains("./README.md\n"), "{text}");
    let omitted: usize = first_line(&text)
        .split("omitted=")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(omitted, 400);
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
    let v3 = anchored(dir.path(), &raw, Some(Profile::Search), Version::V3, &[]);
    assert!(!v3.contains(" files="), "{v3}");

    // A listing that fits the budget is not folded: no name is hidden.
    let fits: String = raw.lines().take(150).map(|l| format!("{l}\n")).collect();
    let text = anchored(dir.path(), &fits, Some(Profile::Search), Version::V4, &[]);
    assert!(!text.contains(" files="), "{text}");
}

/// `ls -R` and `tree` listings fold the same way.
#[test]
fn listing_folds_ls_recursive_and_tree_output() {
    let dir = tempfile::tempdir().unwrap();
    let mut ls = String::from(".:\nREADME.md\nsrc\n\n./src:\n");
    for i in 0..800 {
        ls.push_str(&format!("module_{}.py\n", letters(i)));
    }
    ls.push_str("\n./src/static:\napp.css\n");
    let text = anchored(dir.path(), &ls, Some(Profile::Search), Version::V4, &[]);
    assert!(text.contains("./src/ files=800 ext=py:800\n"), "{text}");
    assert!(text.contains("./src/static:"), "{text}");
    assert!(text.contains("app.css"), "{text}");
    assert_eq!(original(dir.path(), &text), ls.as_bytes());

    let mut tree = String::from(".\n├── Cargo.toml\n└── src\n    ├── main.rs\n    └── handlers\n");
    for i in 0..500 {
        tree.push_str(&format!("        ├── handler_{}.rs\n", letters(i)));
    }
    tree.push_str("        └── mod.rs\n\n3 directories, 504 files\n");
    let text = anchored(dir.path(), &tree, Some(Profile::Search), Version::V4, &[]);
    assert!(
        text.contains("src/handlers/ files=501 ext=rs:501\n"),
        "{text}"
    );
    assert!(text.contains("└── mod.rs"), "{text}");
    assert!(text.contains("3 directories, 504 files"), "{text}");
    assert_eq!(original(dir.path(), &text), tree.as_bytes());
}

/// An install view keeps its warnings, errors and closing summary, never
/// the progress lines between them.
#[test]
fn listing_free_install_views_keep_only_what_decides_the_outcome() {
    let dir = tempfile::tempdir().unwrap();
    let mut raw = String::from("npm install\n");
    for i in 0..300 {
        raw.push_str(&format!(
            "fetch http 200 https://registry.npmjs.org/pkg-{}\n",
            letters(i)
        ));
    }
    raw.push_str("npm WARN deprecated inflight@1.0.6: This module is not supported\n");
    for i in 300..600 {
        raw.push_str(&format!(
            "reify: timing reifyNode:node_modules/pkg-{}\n",
            letters(i)
        ));
    }
    raw.push_str("added 612 packages, and audited 613 packages in 9s\n");
    let output = compress::automatic_profile(
        raw.as_bytes(),
        Some(dir.path().into()),
        Some(Profile::Install),
        None,
        Version::V4,
    )
    .unwrap();
    let text = String::from_utf8(output.into_owned()).unwrap();
    assert!(text.len() <= 2048, "{}", text.len());
    assert!(
        text.contains("npm WARN deprecated inflight@1.0.6"),
        "{text}"
    );
    assert!(text.contains("added 612 packages"), "{text}");
    assert!(
        !text.contains("fetch http 200 https://registry.npmjs.org/pkg-ca\n"),
        "{text}"
    );
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

/// The JSON after a `NAME ` prefix on a view line.
fn json_after(text: &str, prefix: &str) -> serde_json::Value {
    let line = text
        .lines()
        .find_map(|l| l.strip_prefix(prefix))
        .unwrap_or_else(|| panic!("no {prefix:?} line in {text}"));
    serde_json::from_str(line).unwrap()
}

/// Rebuild every row of a v4 table: its columns, then its constants.
fn table_rows(table: &serde_json::Value) -> Vec<serde_json::Value> {
    let columns = table["columns"].as_array().unwrap();
    table["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let mut object: serde_json::Map<String, serde_json::Value> = columns
                .iter()
                .zip(row.as_array().unwrap())
                .map(|(k, v)| (k.as_str().unwrap().to_owned(), v.clone()))
                .collect();
            if let Some(constant) = table["constant"].as_object() {
                object.extend(constant.clone());
            }
            serde_json::Value::Object(object)
        })
        .collect()
}

/// An object whose array of same-keyed objects dominates it is shown as its
/// other fields, then that array as a table whose constant columns appear
/// once; every value survives exactly, nulls and long decimals included.
#[test]
fn json_envelope_with_a_dominant_array_becomes_a_table() {
    let dir = tempfile::tempdir().unwrap();
    let items: Vec<serde_json::Value> = (0..30)
        .map(|i| {
            serde_json::json!({
                "apiVersion": "v1",
                "kind": "Pod",
                "name": format!("api-{}", letters(i)),
                "phase": if i == 17 { "CrashLoopBackOff" } else { "Running" },
                "restarts": i % 4,
                "quota": null,
            })
        })
        .collect();
    let exact: serde_json::Value = serde_json::from_str("900719925474099312345.10").unwrap();
    let document = serde_json::json!({
        "apiVersion": "v1",
        "kind": "List",
        "items": items,
        "metadata": {"resourceVersion": exact},
    });
    let raw = serde_json::to_string_pretty(&document).unwrap();
    let text = compress_v4(dir.path(), raw.as_bytes(), 4096);
    assert!(first_line(&text).ends_with(" complete]\n"), "{text}");
    let envelope = json_after(&text, "envelope ");
    assert_eq!(
        envelope,
        serde_json::json!({"apiVersion": "v1", "kind": "List", "metadata": {"resourceVersion": exact}})
    );
    assert!(text.contains("900719925474099312345.10"), "{text}");
    let table = json_after(&text, "table=/items ");
    assert_eq!(
        table["constant"],
        serde_json::json!({"apiVersion": "v1", "kind": "Pod", "quota": null})
    );
    assert_eq!(table_rows(&table), items);
    assert!(text.contains("CrashLoopBackOff"), "{text}");
    assert_eq!(original(dir.path(), &text), raw.as_bytes());

    let v3 = String::from_utf8(
        compress::automatic_lazy(raw.as_bytes(), Some(dir.path().into()), 4096, Version::V3)
            .unwrap()
            .into_owned(),
    )
    .unwrap();
    assert!(!v3.contains("table=/items"), "{v3}");
}

/// No field is invented: items with different keys are not a table, and an
/// array too large to show whole keeps the line view.
#[test]
fn json_items_that_differ_or_overflow_keep_the_line_view() {
    let dir = tempfile::tempdir().unwrap();
    let mut items: Vec<serde_json::Value> = (0..30)
        .map(|i| serde_json::json!({"name": format!("job-{}", letters(i)), "state": "done"}))
        .collect();
    items[9] = serde_json::json!({"name": "job-j", "state": "failed", "error": "OOMKilled"});
    let raw =
        serde_json::to_string_pretty(&serde_json::json!({"items": items, "count": 30})).unwrap();
    let text = compress_v4(dir.path(), raw.as_bytes(), 4096);
    assert!(!text.contains("table="), "{text}");
    assert!(text.contains("OOMKilled"), "{text}");

    let big: Vec<serde_json::Value> = (0..400)
        .map(|i| serde_json::json!({"name": format!("job-{}", letters(i)), "detail": letters(i * 7919)}))
        .collect();
    let raw = serde_json::to_string_pretty(&serde_json::json!({"items": big})).unwrap();
    let text = compress_v4(dir.path(), raw.as_bytes(), 4096);
    assert!(!text.contains("table="), "{text}");
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

/// A top-level array keeps its table and names its constant columns once.
#[test]
fn json_tables_name_constant_columns_once() {
    let dir = tempfile::tempdir().unwrap();
    let rows: Vec<serde_json::Value> = (0..80)
        .map(|i| serde_json::json!({"id": i, "region": "eu-west-3", "status": "ok", "owner": "platform-team"}))
        .collect();
    let raw = rows
        .iter()
        .map(|r| r.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let text = compress_v4(dir.path(), raw.as_bytes(), 4096);
    let table = json_after(&text, "table ");
    assert_eq!(table["columns"], serde_json::json!(["id"]));
    assert_eq!(table_rows(&table), rows);
    assert!(table.get("indices").is_none(), "{text}");
    assert_eq!(text.matches("eu-west-3").count(), 1, "{text}");
}

/// Found on a real `gh api` response: a view that cuts a line is not
/// complete, whatever it omits.
#[test]
fn a_cut_line_makes_the_header_partial() {
    let dir = tempfile::tempdir().unwrap();
    let giant = format!("start\n{}\nend\n", "x".repeat(20000));
    let text = compress_v4(dir.path(), giant.as_bytes(), 4096);
    assert!(text.contains("text_truncated bytes=20000"), "{text}");
    let header = first_line(&text);
    assert!(!header.ends_with(" complete]\n"), "{header}");
    assert!(header.contains(" abridged=1;"), "{header}");
}

/// Found on real `gh api` responses (165 to 265 KB on one line): their
/// objects' short fields become a table, nested and long fields are named,
/// and the rows that do not fit are counted.
#[test]
fn json_minified_api_responses_show_a_projected_table() {
    let dir = tempfile::tempdir().unwrap();
    let pulls: Vec<serde_json::Value> = (0..40)
        .map(|i| {
            serde_json::json!({
                "number": 3500 + i,
                "state": if i == 7 { "closed" } else { "open" },
                "title": format!("printer: fix case {}", letters(i)),
                "locked": false,
                "body": "long description ".repeat(90),
                "user": {"login": format!("dev-{}", letters(i)), "id": i},
                "labels": [{"name": "bug"}],
            })
        })
        .collect();
    let mut pulls = pulls;
    pulls[36]["title"] = serde_json::json!("error: release build failed");
    let raw = serde_json::to_string(&pulls).unwrap();
    let text = compress_v4(dir.path(), raw.as_bytes(), 4096);
    let table = json_after(&text, "table ");
    // The row carrying a diagnostic comes first, wherever it is.
    assert!(text.contains("error: release build failed"), "{text}");
    assert_eq!(
        table["columns"],
        serde_json::json!(["number", "state", "title"])
    );
    assert_eq!(table["constant"], serde_json::json!({"locked": false}));
    assert_eq!(
        table["omitted_columns"],
        serde_json::json!(["body", "labels", "user"])
    );
    let rows = table["rows"].as_array().unwrap();
    assert_eq!(
        rows[0],
        serde_json::json!([3500, "open", "printer: fix case a"])
    );
    assert_eq!(rows[7][1], "closed");
    let header = first_line(&text);
    assert!(
        header.contains(&format!(
            " omitted={} abridged={};",
            40 - rows.len(),
            rows.len()
        )) || (rows.len() == 40 && header.contains(" omitted=0 abridged=40;")),
        "{header}"
    );
    assert_eq!(original(dir.path(), &text), raw.as_bytes());

    // The envelope of a search response keeps its scalars.
    let search =
        serde_json::json!({"total_count": 251, "incomplete_results": false, "items": pulls});
    let raw = search.to_string();
    let text = compress_v4(dir.path(), raw.as_bytes(), 4096);
    assert_eq!(
        json_after(&text, "envelope "),
        serde_json::json!({"total_count": 251, "incomplete_results": false})
    );
    assert!(
        json_after(&text, "table=/items ")["rows"].is_array(),
        "{text}"
    );
}

/// Found on a real `go test -v`: a panicking test points at the crash site,
/// not at the Go toolchain's `testing.go`.
#[test]
fn failures_of_a_go_panic_point_outside_the_toolchain() {
    let dir = tempfile::tempdir().unwrap();
    let raw = format!(
        "{}=== RUN   TestRotate\n--- FAIL: TestRotate (0.00s)\npanic: runtime error: invalid memory address or nil pointer dereference [recovered, repanicked]\n\ngoroutine 95 [running]:\ntesting.tRunner.func1.2({{0x1, 0x2}})\n\t/opt/homebrew/Cellar/go/1.27.2/libexec/src/testing/testing.go:2125 +0x1a0\npanic({{0x1, 0x2}})\n\t/opt/homebrew/Cellar/go/1.27.2/libexec/src/runtime/panic.go:859 +0x120\nexample.com/gmod.(*S).Get(...)\n\t/work/gmod/calc.go:7\nexample.com/gmod.TestRotate(0x5?)\n\t/work/gmod/calc_test.go:72 +0x8\nFAIL\texample.com/gmod\t0.007s\nFAIL\n",
        passes(600)
    );
    let text = test_view(dir.path(), &raw, Version::V4);
    assert_eq!(
        action_line(&text),
        Some("failed=1: TestRotate (/work/gmod/calc.go:7)"),
        "{text}"
    );
}

/// Found on a real `rg -n 'fn ' src`: the search's own pattern is on every
/// line, so it singles nothing out and must not spend the anchored share
/// that the prompt's term needs.
#[test]
fn anchor_terms_on_most_lines_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let names: Vec<String> = (0..30).map(|f| format!("src/m{}.rs", letters(f))).collect();
    let files: Vec<(&str, usize)> = names.iter().map(|n| (n.as_str(), 40)).collect();
    let mut raw = grep_output(&files);
    raw = raw.replacen(
        "src/mbd.rs:206:",
        "src/mbd.rs:206:    fn resolve_prefix() {} //",
        1,
    );
    let plain = anchored(
        dir.path(),
        &raw,
        Some(Profile::Search),
        Version::V4,
        &["let "],
    );
    assert!(!plain.contains("resolve_prefix"), "{plain}");
    let focused = anchored(
        dir.path(),
        &raw,
        Some(Profile::Search),
        Version::V4,
        &["let ", "resolve_prefix"],
    );
    assert!(focused.contains("resolve_prefix"), "{focused}");
}

/// Found on a real `pip install`: the line saying what was installed is the
/// outcome even when warnings follow it.
#[test]
fn listing_free_install_views_keep_the_installed_summary() {
    let dir = tempfile::tempdir().unwrap();
    let mut raw = String::new();
    for i in 0..200 {
        raw.push_str(&format!(
            "Collecting pkg-{0}\n  Downloading pkg_{0}-1.0-py3-none-any.whl (64 kB)\n",
            letters(i)
        ));
    }
    raw.push_str("Installing collected packages: pkg-a, pkg-b\n");
    raw.push_str("Successfully installed pkg-a-1.0 pkg-b-1.0\n");
    raw.push_str(
        "WARNING: You are using pip version 21.2.4; however, version 26.0.1 is available.\n",
    );
    raw.push_str(
        "You should consider upgrading via the 'python3 -m pip install --upgrade pip' command.\n",
    );
    let output = compress::automatic_profile(
        raw.as_bytes(),
        Some(dir.path().into()),
        Some(Profile::Install),
        None,
        Version::V4,
    )
    .unwrap();
    let text = String::from_utf8(output.into_owned()).unwrap();
    assert!(
        text.contains("Successfully installed pkg-a-1.0 pkg-b-1.0"),
        "{text}"
    );
    assert!(
        text.contains("WARNING: You are using pip version"),
        "{text}"
    );
    assert!(!text.contains("Downloading pkg_ca"), "{text}");
}

/// Found on real `gh api` responses: one link column is kept and the other
/// links are named, so the rows that matter fit.
#[test]
fn json_projections_keep_one_link_column() {
    let dir = tempfile::tempdir().unwrap();
    let issues: Vec<serde_json::Value> = (0..30)
        .map(|i| {
            let base = format!("https://api.github.com/repos/acme/app/issues/{i}");
            serde_json::json!({
                "number": i,
                "title": format!("issue {}", letters(i)),
                "url": base,
                "html_url": format!("https://github.com/acme/app/issues/{i}"),
                "comments_url": format!("{base}/comments"),
                "events_url": format!("{base}/events"),
                "user": {"login": "dev", "bio": "x".repeat(1500)},
            })
        })
        .collect();
    let raw = serde_json::to_string(&issues).unwrap();
    let text = compress_v4(dir.path(), raw.as_bytes(), 4096);
    let table = json_after(&text, "table ");
    assert_eq!(
        table["columns"],
        serde_json::json!(["html_url", "number", "title"])
    );
    assert_eq!(
        table["omitted_columns"],
        serde_json::json!(["comments_url", "events_url", "url", "user"])
    );
    assert_eq!(table["rows"].as_array().unwrap().len(), 30, "{text}");
}

/// Found on a live Grep result: 600 lines sharing a template fold into one
/// `similar=600` line whose members differ (their file names), so the view
/// is not complete; an exact `repeat=N` hides nothing.
#[test]
fn a_template_fold_makes_the_header_partial() {
    let dir = tempfile::tempdir().unwrap();
    let raw: String = (0..600)
        .map(|i| {
            format!(
                "src/mod{}.rs:{}:fn handler() {{ refresh_token({i}); }}\n",
                i / 20,
                i % 20
            )
        })
        .collect();
    let text = compress_v4(dir.path(), raw.as_bytes(), 4096);
    assert!(text.contains(" similar="), "{text}");
    assert!(!first_line(&text).ends_with(" complete]\n"), "{text}");
    assert!(first_line(&text).contains(" abridged="), "{text}");
}
