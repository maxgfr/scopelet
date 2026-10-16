//! Compact view contracts: one header line with a 12-character artifact ID
//! and no footer, labels by line number alone for the one `input` text,
//! tables as CSV rows, cleaned display copies, cut oversized lines, folded
//! repetitions and diagnostic-first selection, with the original bytes always
//! recoverable from the store.
use scopelet::{
    commands::Profile,
    compress,
    model::{Dataset, Record},
    store::Store,
};
use serde_json::{Value, json};

/// The automatic view of `raw` at `budget`, without a profile.
fn compress(dir: &std::path::Path, raw: &[u8], budget: usize) -> String {
    let output = compress::automatic_decision(raw, Some(dir.to_path_buf()), None, Some(budget))
        .map(|(view, _)| view)
        .unwrap();
    assert!(output.len() <= budget, "{} bytes over budget", output.len());
    String::from_utf8(output.into_owned()).unwrap()
}

/// The automatic view of `raw` for `profile`, ranked by `anchors`.
fn anchored(
    dir: &std::path::Path,
    raw: &str,
    profile: Option<Profile>,
    anchors: &[&str],
) -> String {
    let anchors: Vec<String> = anchors.iter().map(|s| s.to_string()).collect();
    let output = compress::automatic_anchored(
        "cli",
        raw.as_bytes(),
        Some(dir.to_path_buf()),
        profile,
        None,
        &anchors,
    );
    String::from_utf8(output.into_owned()).unwrap()
}

/// The 12 hex characters a header shows.
fn short_id(text: &str) -> &str {
    text.strip_prefix("[scopelet v5 ")
        .and_then(|rest| rest.get(..12))
        .filter(|id| id.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap_or_else(|| panic!("no view header: {text}"))
}

fn original(dir: &std::path::Path, text: &str) -> Vec<u8> {
    let store = Store::open(Some(dir.to_path_buf())).unwrap();
    let artifact = store.resolve(short_id(text)).unwrap();
    let data: Dataset = store.dataset(&artifact).unwrap();
    store.get(&data.snapshots[0].blob).unwrap()
}

fn first_line(text: &str) -> &str {
    text.split_inclusive('\n').next().unwrap()
}

/// A digit-free word for each number, so no template folds two of them.
fn letters(i: usize) -> String {
    i.to_string()
        .bytes()
        .map(|b| (b'a' + (b - b'0')) as char)
        .collect()
}

/// Lines no template folds: each carries a distinct word of letters.
fn distinct_lines(count: usize) -> String {
    (0..count)
        .map(|i| format!("{} unit ready\n", letters(i)))
        .collect()
}

fn noise_lines(count: usize) -> String {
    (0..count)
        .map(|i| format!("compiling unit {i}\n"))
        .collect()
}

// Header and envelope.

#[test]
fn a_complete_view_has_one_short_header_line_and_no_footer() {
    let dir = tempfile::tempdir().unwrap();
    let raw = format!(
        "start\n{}fatal error: rare root cause\nend\n",
        "error: retry failed\nworking\n".repeat(1000)
    );
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    let header = first_line(&text);
    assert_eq!(
        header,
        format!("[scopelet v5 {} complete]\n", short_id(&text))
    );
    assert!(header.len() <= 40, "{} header bytes", header.len());
    assert_eq!(text.matches("[scopelet ").count(), 1, "{text}");
    assert!(
        text.contains("\n2002 fatal error: rare root cause\n"),
        "{text}"
    );
    assert!(!text.contains("input:"), "{text}");
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

/// A partial view counts its omissions and carries no recovery syntax: the
/// session guidance gives it. `expand` resolves the 12 characters like any
/// unique prefix.
#[test]
fn a_partial_view_counts_its_omissions_in_the_header() {
    let dir = tempfile::tempdir().unwrap();
    let raw = distinct_lines(3000);
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    let header = first_line(&text);
    let id = short_id(&text);
    let rest = header
        .strip_prefix(&format!("[scopelet v5 {id} omitted="))
        .unwrap_or_else(|| panic!("{header}"));
    let (count, tail) = rest.split_once(';').unwrap();
    assert!(count.parse::<usize>().unwrap() > 0, "{header}");
    assert_eq!(tail, " gaps are not absence]\n");
    assert!(header.len() <= 70, "{} header bytes", header.len());
    assert!(!text.contains("display_complete"), "{text}");
    assert!(text.lines().nth(1).unwrap().starts_with("1-"), "{text}");
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
    let found = assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("scopelet"))
        .args(["--cache-dir", dir.path().to_str().unwrap(), "expand"])
        .args([id, "--find", "unit ready"])
        .output()
        .unwrap();
    assert!(found.status.success(), "{found:?}");
}

/// An explicit compact query keeps its `source:` labels, and says when its
/// scan was incomplete, in the header only.
#[test]
fn an_explicit_query_keeps_its_sources_and_flags_an_incomplete_scan() {
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
    let complete = compress::compact(&data, &store, 4096).unwrap();
    assert_eq!(
        complete,
        format!(
            "[scopelet v5 {} complete]\nsrc/lib.rs:1 fn main() {{}}\n",
            short_id(&complete)
        )
    );
    data.incomplete("synthetic cap".into());
    let partial_scan = compress::compact(&data, &store, 4096).unwrap();
    let id = short_id(&partial_scan);
    assert_eq!(
        first_line(&partial_scan),
        format!("[scopelet v5 {id} scan_complete=false complete]\n")
    );
    assert!(store.resolve(id).is_ok());
}

/// Scopelet's own output is never compressed again, but only a marker at the
/// start of a line is one: source code quoting it is ordinary text.
#[test]
fn only_a_marker_at_a_line_start_means_already_compressed() {
    let dir = tempfile::tempdir().unwrap();
    let quoting: String = (0..400)
        .map(|i| format!("    let header_{i} = format!(\"[scopelet v5 {{id}}\");\n"))
        .collect();
    let text = compress(dir.path(), quoting.as_bytes(), 4096);
    assert!(text.starts_with("[scopelet v5 "), "{text}");
    // A view, or a note, at the start of a line passes through.
    let view = format!("{text}{}", "more\n".repeat(1000));
    let again =
        compress::automatic_decision(view.as_bytes(), Some(dir.path().into()), None, Some(4096))
            .map(|(view, _)| view)
            .unwrap();
    assert_eq!(again.as_ref(), view.as_bytes());
    let note = format!(
        "{}[scopelet capture_complete=false exit_code=1; saved output may be partial]\n",
        "line\n".repeat(1000)
    );
    let kept =
        compress::automatic_decision(note.as_bytes(), Some(dir.path().into()), None, Some(4096))
            .map(|(view, _)| view)
            .unwrap();
    assert_eq!(kept.as_ref(), note.as_bytes());
    // Views written before 1.0 are recognized alike.
    for old in [
        "[scopelet v4 0123456789abcdef complete]\n",
        "[scopelet compact-v3 artifact:0 x]\n",
    ] {
        let padded = format!("{old}{}", "padding line\n".repeat(400));
        let again = compress::automatic_decision(
            padded.as_bytes(),
            Some(dir.path().into()),
            None,
            Some(4096),
        )
        .map(|(view, _)| view)
        .unwrap();
        assert_eq!(again.as_ref(), padded.as_bytes());
    }
}

/// Found on a real `gh api` response: a view that cuts a line is not
/// complete, whatever it omits.
#[test]
fn a_cut_line_makes_the_header_partial() {
    let dir = tempfile::tempdir().unwrap();
    let giant = format!("start\n{}\nend\n", "x".repeat(20000));
    let text = compress(dir.path(), giant.as_bytes(), 4096);
    assert!(text.contains("text_truncated bytes=20000"), "{text}");
    let header = first_line(&text);
    assert!(!header.ends_with(" complete]\n"), "{header}");
    assert!(header.contains(" abridged=1;"), "{header}");
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
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    assert!(text.contains(" similar="), "{text}");
    assert!(!first_line(&text).ends_with(" complete]\n"), "{text}");
    assert!(first_line(&text).contains(" abridged="), "{text}");
}

/// A repeat reference carries the short ID and the facts, nothing more.
#[test]
fn a_repeat_reference_is_short() {
    let dir = tempfile::tempdir().unwrap();
    let log: String = (0..400)
        .map(|i| format!("worker {i} finished batch\n"))
        .collect();
    let event = json!({"hook_event_name":"PostToolUse","session_id":"r1","tool_name":"Bash","tool_input":{"command":"npm test"},"tool_response":{"stdout":log,"stderr":"","interrupted":false,"isImage":false}});
    let run = || -> String {
        hook(dir.path(), "claude", event.clone())["hookSpecificOutput"]["updatedToolOutput"]
            ["stdout"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let first = run();
    let second = run();
    assert_eq!(
        second,
        format!(
            "[scopelet v5 {} repeat: identical to output already shown in this conversation (400 lines)]\n",
            short_id(&first)
        )
    );
}

// Display copies and cut lines.

#[test]
fn colored_diagnostics_are_displayed_clean_and_recovered_with_their_codes() {
    let dir = tempfile::tempdir().unwrap();
    let raw = format!(
        "{}\x1b[31mFAILED\x1b[0m tests::alpha\n{}",
        "test ok\n".repeat(1500),
        "test ok\n".repeat(1500)
    );
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    assert!(text.starts_with("[scopelet v5 "));
    assert!(text.contains(" FAILED tests::alpha\n"), "{text}");
    assert!(!text.contains('\x1b'), "{text}");
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

#[test]
fn progress_bars_show_only_their_final_state() {
    let dir = tempfile::tempdir().unwrap();
    let raw = format!(
        "start\n{}10%\r50%\r100% done\nend\n",
        "building\n".repeat(1000)
    );
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    assert!(text.contains("\n1002 100% done\n"), "{text}");
    assert!(!text.contains("10%"), "{text}");
    assert!(text.contains("\n2-1001 repeat=1000 building\n"), "{text}");
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

#[test]
fn oversized_lines_are_cut_behind_a_marker_and_recovered_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let giant = "é".repeat(6000);
    let text = compress(dir.path(), giant.as_bytes(), 4096);
    assert!(text.contains("\n1 text_truncated bytes=12000 éé"), "{text}");
    assert!(text.len() * 100 <= giant.len() * 15, "{} bytes", text.len());
    assert_eq!(original(dir.path(), &text), giant.as_bytes());

    let raw = format!(
        "{}{}\nerror: after the giant line\nend\n",
        "ok\n".repeat(200),
        "x".repeat(20000)
    );
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    assert!(
        text.contains("\n201 text_truncated bytes=20000 xxx"),
        "{text}"
    );
    assert!(
        text.contains("\n202 error: after the giant line\n"),
        "{text}"
    );
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

/// A JSON record is never split, but a document no whole record of which
/// fits (one large object) is shown by its source lines instead of passing
/// through whole: labels are absolute lines of the original text.
#[test]
fn oversized_json_records_fall_back_to_lines() {
    let dir = tempfile::tempdir().unwrap();
    let pods: Vec<_> = (0..300)
        .map(|i| {
            let state = if i == 141 {
                "CrashLoopBackOff"
            } else {
                "Running"
            };
            json!({"name": format!("api-{i}"), "restarts": i % 3, "state": state})
        })
        .collect();
    let raw = serde_json::to_string_pretty(&json!({"kind": "List", "items": pods})).unwrap();
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    assert!(text.contains("\"CrashLoopBackOff\""), "{text}");
    assert!(text.lines().nth(1).unwrap().starts_with('1'), "{text}");
    assert!(!text.contains("#record="), "{text}");
    assert_eq!(original(dir.path(), &text), raw.as_bytes());

    // A minified document is one line: it is cut behind a marker.
    let raw = json!({"title": "keep me", "value": "x".repeat(50000)}).to_string();
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    assert!(text.contains("\n1 text_truncated bytes="), "{text}");
    assert!(text.contains("\"title\":\"keep me\""), "{text}");
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

/// Selection memory grows with the line count, so automatic compression has
/// a line limit.
#[test]
fn automatic_compression_has_a_line_limit() {
    let dir = tempfile::tempdir().unwrap();
    let raw: String = (0..150_000)
        .map(|i| format!("{i} ok\n"))
        .chain(std::iter::once("error: late failure\n".to_owned()))
        .collect();
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    assert!(text.contains("error: late failure"), "{text}");
    let over: String = "ok\n".repeat(compress::AUTO_MAX_LINES + 1);
    let output =
        compress::automatic_decision(over.as_bytes(), Some(dir.path().into()), None, Some(4096))
            .map(|(view, _)| view)
            .unwrap();
    assert_eq!(output.as_ref(), over.as_bytes());
}

// Folding.

#[test]
fn dispersed_repetitions_fold_into_their_first_occurrence() {
    let dir = tempfile::tempdir().unwrap();
    let raw = format!(
        "start\n{}fatal error: rare root cause\nend\n",
        "error: retry failed\nworking\n".repeat(1000)
    );
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    for expected in [
        "\n1 start\n",
        "\n2 repeat=1000 last=2000 error: retry failed\n",
        "\n3 repeat=1000 last=2001 working\n",
        "\n2002 fatal error: rare root cause\n",
        "\n2003 end\n",
        " complete]\n",
    ] {
        assert!(text.contains(expected), "{expected:?} missing in:\n{text}");
    }
    assert!(text.len() < 400, "{} bytes:\n{text}", text.len());
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

#[test]
fn template_folding_keeps_the_rare_line_visible_among_noise() {
    let dir = tempfile::tempdir().unwrap();
    let noise: String = (0..1000)
        .map(|i| format!("progress {i:04}: {}\n", "unchanged ".repeat(12)))
        .collect();
    let half = noise.len() / 2;
    let raw = format!(
        "{}receipt audit-7139 amount=47 EUR{}\n{}",
        &noise[..half],
        " padding".repeat(20),
        &noise[half..]
    );
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    assert!(
        text.contains("\n1 similar=1000 last=1001 progress 0000: unchanged "),
        "{text}"
    );
    assert!(
        text.contains("\n501 receipt audit-7139 amount=47 EUR padding"),
        "{text}"
    );
    assert!(first_line(&text).contains(" omitted=0 "), "{text}");
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

/// Lines that differ only by their numbers are the evidence when the text
/// fits: only identical lines fold. A text too large still folds them.
#[test]
fn similar_lines_fold_only_when_the_text_does_not_fit() {
    let dir = tempfile::tempdir().unwrap();
    let run = |failures: usize| {
        format!(
            "{}case runs 96 failures {failures}\n",
            "Terminated: 15\n".repeat(90)
        )
    };
    let raw = format!("{}{}", run(5), run(3));
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    assert!(text.contains("case runs 96 failures 5\n"), "{text}");
    assert!(text.contains("case runs 96 failures 3\n"), "{text}");
    assert!(!text.contains("similar="), "{text}");
    assert!(text.contains(" repeat=180 "), "{text}");
    assert_eq!(
        first_line(&text),
        format!("[scopelet v5 {} complete]\n", short_id(&text))
    );

    let raw: String = (0..3000)
        .map(|i| format!("case runs {i} failures 0\n"))
        .collect();
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    assert!(text.contains(" similar=3000 "), "{text}");
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

/// A diagnostic word inside a path, a module path or a file name is a name,
/// not an outcome: `src/errors/e1.rs` and `Error::new` are ordinary lines that
/// fold, and the real error is not crowded out by them.
#[test]
fn diagnostic_words_inside_paths_and_names_are_not_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let names: String = (0..300)
        .map(|i| format!("checked src/errors/e{i}.rs, failure_{i}.py and Error::new({i})\n"))
        .collect();
    let raw = format!("start\n{names}error: real failure here\n{names}end\n");
    let text = compress(dir.path(), raw.as_bytes(), 1024);
    assert!(text.contains(" error: real failure here\n"), "{text}");
    assert!(text.contains("similar=600"), "{text}");
    assert_eq!(text.matches("checked src/errors").count(), 1, "{text}");
}

/// A flood of one error differing only by its timestamp is one diagnostic
/// repeated: it folds, its counters stay visible, and the rare fatal line
/// among it is shown.
#[test]
fn timestamped_diagnostics_fold_and_keep_their_identifiers() {
    let dir = tempfile::tempdir().unwrap();
    let mut raw = String::new();
    for i in 0..1200 {
        let ts = format!(
            "2026-09-30T12:{:02}:{:02}.{:03}Z",
            i / 60 % 60,
            i % 60,
            i * 7 % 1000
        );
        if i == 900 {
            raw.push_str(&format!("{ts} FATAL migration 0042 failed: no relation\n"));
        } else {
            raw.push_str(&format!(
                "{ts} ERROR conn refused 10.0.3.7:5432 (attempt {}/2)\n",
                i % 2 + 1
            ));
        }
    }
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    assert!(
        text.contains("FATAL migration 0042 failed: no relation"),
        "{text}"
    );
    assert!(text.contains("(attempt 1/2)"), "{text}");
    assert!(text.contains("(attempt 2/2)"), "{text}");
    assert!(text.contains("similar=600"), "{text}");
    assert!(first_line(&text).contains(" omitted=0 "), "{text}");
}

// Diagnostic-first selection.

#[test]
fn crowded_distinct_diagnostics_keep_first_and_final_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let middle = (0..1000)
        .map(|i| format!("error: case {i} failed\n"))
        .collect::<String>();
    let raw = format!("running 1000 tests\n{middle}final status: 1000 failed; exit=7\n");
    let text = compress(dir.path(), raw.as_bytes(), 1024);
    assert!(text.contains("running 1000 tests\n"), "{text}");
    assert!(text.contains(" error: case 0 failed\n"), "{text}");
    assert!(text.contains(" error: case 999 failed\n"), "{text}");
    assert!(
        text.contains("final status: 1000 failed; exit=7\n"),
        "{text}"
    );
    assert!(first_line(&text).contains(" omitted="), "{text}");
}

#[test]
fn diagnostic_vocabulary_and_frames_survive_noise() {
    let dir = tempfile::tempdir().unwrap();
    let facts = [
        "error[E0308]: mismatched types",
        "error TS2322: Type 'string' is not assignable",
        "npm ERR! code ELIFECYCLE",
        "FAILED tests/test_alpha.py::test_beta - AssertionError",
        "    at Object.<anonymous> (/app/src/index.js:10:5)",
        "  File \"/app/main.py\", line 12, in <module>",
        "fatal: not a git repository",
        "✗ 3 of 12 checks",
        "    TypeError: Cannot read properties of undefined (reading 'id')",
        "Caused by: java.lang.NullPointerException: name is null",
        "\tat com.acme.billing.Invoice.total(Invoice.java:88)",
        // Java identifiers may be Unicode: a frame stays a frame.
        "\tat com.acme.société.Facture.total(Facture.java:91)",
        "--- FAIL: TestRefresh (0.00s)",
        "\t/work/app/auth/session.go:88 +0x1d",
        "  --> src/cache.rs:17:9",
        "✖ 3 problems (1 error, 2 warnings)",
    ];
    // Foldable noise leaves room for anything; distinct noise only leaves
    // room for lines ranked as diagnostics or their context.
    let distinct = |block: usize| -> String {
        distinct_lines(200 * (block + 1))
            .lines()
            .skip(200 * block)
            .map(|line| format!("{line}\n"))
            .collect()
    };
    let foldable = |_| noise_lines(200);
    let generators: [&dyn Fn(usize) -> String; 2] = [&foldable, &distinct];
    for noise in generators {
        let mut raw = String::new();
        for (block, fact) in facts.iter().enumerate() {
            raw.push_str(&noise(block));
            raw.push_str(fact);
            raw.push('\n');
        }
        raw.push_str(&noise(facts.len()));
        let text = compress(dir.path(), raw.as_bytes(), 4096);
        for fact in facts {
            assert!(text.contains(fact), "{fact:?} missing in:\n{text}");
        }
        assert_eq!(original(dir.path(), &text), raw.as_bytes());
    }
}

#[test]
fn warnings_do_not_evict_errors_from_a_small_budget() {
    let dir = tempfile::tempdir().unwrap();
    let warnings: String = (0..300)
        .map(|i| format!("warning: unused variable `w{i}`\n"))
        .collect();
    let errors: String = (0..300)
        .map(|i| format!("error: cannot find value `v{i}` in this scope\n"))
        .collect();
    let raw = format!("compiling\n{warnings}{errors}done\n");
    let text = compress(dir.path(), raw.as_bytes(), 1024);
    let errors_shown = text.matches("error: cannot find").count();
    let warnings_shown = text.matches("warning: unused").count();
    assert!(errors_shown >= 5, "{errors_shown} errors in:\n{text}");
    assert!(warnings_shown <= 1, "{warnings_shown} warnings in:\n{text}");
}

/// The rustc build of a live task: an error among 2,000 warnings, each with
/// its own `-->` location. The error's location ranks with the error, so it
/// is not lost among the warnings' locations.
#[test]
fn an_error_keeps_its_location_among_a_flood_of_warnings() {
    let dir = tempfile::tempdir().unwrap();
    for count in [2000, 300] {
        let mut raw = String::new();
        for i in 0..count {
            raw.push_str(&format!("warning: unused variable `x_{}`\n", letters(i)));
            let module = if i % 97 == 0 {
                "src/ledger/settled.rs".to_owned()
            } else {
                format!("src/mod_{}.rs", letters(i % 40))
            };
            raw.push_str(&format!("  --> {module}:{}:9\n", i % 300 + 1));
            if i == count * 2 / 3 {
                raw.push_str("error[E0308]: mismatched types\n  --> src/ledger/settle.rs:88:17\n");
            }
        }
        raw.push_str(&format!(
            "error: could not compile `ledger` (lib) due to 1 previous error; {count} warnings emitted\n"
        ));
        let output = compress::automatic_decision(
            raw.as_bytes(),
            Some(dir.path().into()),
            Some(Profile::Tests),
            Some(4096),
        )
        .map(|(view, _)| view)
        .unwrap();
        let text = String::from_utf8(output.into_owned()).unwrap();
        assert!(text.len() <= 4096, "{}", text.len());
        assert!(text.contains("error[E0308]: mismatched types"), "{text}");
        assert!(
            text.contains("src/ledger/settle.rs:88:17"),
            "{count}: {text}"
        );
        assert_eq!(original(dir.path(), &text), raw.as_bytes());
    }
}

#[test]
fn the_final_summary_survives_a_flood_of_failures() {
    let dir = tempfile::tempdir().unwrap();
    let failures: String = (0..500)
        .map(|i| format!("test module::case_{i} ... FAILED\n"))
        .collect();
    let raw = format!(
        "running 500 tests\n{failures}\nfailures:\n\ntest result: FAILED. 0 passed; 500 failed; 0 ignored\n\nerror: test failed, to rerun pass `--lib`\n"
    );
    let text = compress(dir.path(), raw.as_bytes(), 1024);
    assert!(
        text.contains("test result: FAILED. 0 passed; 500 failed; 0 ignored\n"),
        "{text}"
    );
    assert!(text.contains("test module::case_0 ... FAILED\n"), "{text}");
    assert!(
        text.contains("test module::case_499 ... FAILED\n"),
        "{text}"
    );
}

#[test]
fn contiguous_selected_lines_form_a_range_block_with_derivable_labels() {
    let dir = tempfile::tempdir().unwrap();
    let context = [
        "  --> src/lib.rs:12:9",
        "   |",
        "12 |     let total = alpha + beta;",
        "   |                         ^^^^ not found in this scope",
        "   |",
        "help: a local variable with a similar name exists",
        "   |",
        "12 |     let total = alpha + gamma;",
    ];
    let raw = format!(
        "{}error[E0425]: cannot find value\n{}\n{}",
        noise_lines(500),
        context.join("\n"),
        noise_lines(500)
    );
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    let lines: Vec<&str> = text.lines().collect();
    let header = lines
        .iter()
        .position(|l| l.starts_with("501-"))
        .unwrap_or_else(|| panic!("no range block in:\n{text}"));
    let (a, b) = lines[header].split_once('-').unwrap();
    let (a, b): (usize, usize) = (a.parse().unwrap(), b.parse().unwrap());
    assert!(b - a >= 2, "{text}");
    let source: Vec<&str> = raw.lines().collect();
    for (k, line) in lines[header + 1..=header + 1 + b - a].iter().enumerate() {
        let body = line
            .strip_prefix(' ')
            .unwrap_or_else(|| panic!("{line:?} in:\n{text}"));
        assert_eq!(body, source[a - 1 + k], "line {} in:\n{text}", a + k);
    }
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

#[test]
fn ordinary_lines_are_capped_only_when_diagnostics_cannot_show_them_all() {
    let dir = tempfile::tempdir().unwrap();
    let raw = format!(
        "{}error: unit zulu failed\n{}",
        distinct_lines(500),
        distinct_lines(500)
    );
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    assert!(text.contains(" error: unit zulu failed\n"), "{text}");
    assert!(!first_line(&text).contains(" omitted=0"), "{text}");
    assert!(text.len() < 2048, "{} bytes:\n{text}", text.len());
    assert!(text.contains("\n1-"), "{text}");

    // An ordinary listing without diagnostics still fills the budget.
    let listing = distinct_lines(1000);
    let text = compress(dir.path(), listing.as_bytes(), 4096);
    assert!(text.len() > 3500, "{} bytes:\n{text}", text.len());
    assert!(!first_line(&text).contains(" omitted=0"), "{text}");
}

/// Deterministic pseudo-random generator; no dependency, reproducible failures.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() as usize) % n
    }
}

/// A mix of the shapes the view treats specially: repeats, escapes,
/// carriage-return rewrites, oversized lines, diagnostics, non-ASCII and
/// near-identical lines.
fn generated_input(seed: u64, lines: usize) -> String {
    let mut rng = Rng(seed);
    let mut out = String::new();
    for i in 0..lines {
        match rng.below(9) {
            0 => out.push_str("building the workspace\n"),
            1 => out.push_str(&format!("progress {:05}: {}\n", i, "unchanged ".repeat(3))),
            2 => out.push_str(&format!("\x1b[31merror\x1b[0m: case {i} failed\n")),
            3 => out.push_str(&format!("  at handler (/app/src/i{i}.js:{i}:5)\n")),
            4 => out.push_str(&format!("10%\r60%\rdone {i}\n")),
            5 => out.push_str(&format!("warning: unused symbol s{i}\n")),
            6 => out.push_str(&format!("café {} é\n", "é".repeat(rng.below(40)))),
            7 => out.push_str(&format!("{}\n", "w".repeat(rng.below(6000)))),
            _ => out.push_str(&format!("record {:08x} ready\n", rng.next())),
        }
    }
    out
}

/// A unit label: a line number or range, then its metadata tokens.
fn label_numbers(label: &str) -> bool {
    !label.is_empty()
        && label
            .split('-')
            .all(|n| n.bytes().all(|b| b.is_ascii_digit()) && !n.is_empty())
}

/// Every label in a view resolves to the source line it names, and the text
/// beside it is exactly what a terminal would have shown for that line (or a
/// prefix of it when the line is marked truncated). A label that pointed
/// somewhere else would be a lie the reader cannot detect.
#[test]
fn every_label_resolves_to_the_line_it_names() {
    let (mut views, mut units) = (0usize, 0usize);
    for seed in 0..40u64 {
        let raw = generated_input(seed, 60 + (seed as usize % 400));
        if raw.len() <= 2048 {
            continue;
        }
        let dir = tempfile::tempdir().unwrap();
        let output =
            compress::automatic_decision(raw.as_bytes(), Some(dir.path().into()), None, Some(4096))
                .map(|(view, _)| view)
                .unwrap();
        let text = std::str::from_utf8(&output).unwrap();
        if text == raw {
            continue; // Passthrough keeps the original bytes; nothing to check.
        }
        assert_eq!(original(dir.path(), text), raw.as_bytes(), "seed {seed}");
        let source: Vec<String> = raw.split_inclusive('\n').map(terminal_view).collect();
        let mut block: Option<(usize, usize)> = None;
        for line in text.lines() {
            if line.starts_with("[scopelet ") {
                continue;
            }
            if let Some(rest) = line.strip_prefix(' ') {
                let (at, end) = block.expect("indented line outside a range block");
                assert!(at <= end, "seed {seed}: block overran its header");
                assert_eq!(source[at - 1], rest, "seed {seed}: block line {at}");
                block = Some((at + 1, end));
                continue;
            }
            let (numbers, body) = match line.split_once(' ') {
                Some((label, body)) => (label, Some(body)),
                None => (line, None),
            };
            assert!(label_numbers(numbers), "seed {seed}: unlabelled {line:?}");
            if body.is_none() {
                let (a, b) = numbers.split_once('-').expect("a block header is a range");
                block = Some((a.parse().unwrap(), b.parse().unwrap()));
                continue;
            }
            let mut shown = body.unwrap();
            let start: usize = numbers
                .split_once('-')
                .map_or(numbers, |(a, _)| a)
                .parse()
                .unwrap();
            // Strip the metadata tokens the notation may place before the text.
            let mut truncated = None;
            loop {
                let token = shown.split_once(' ').map_or(shown, |(t, _)| t);
                let counted = |t: &str, name: &str| {
                    t.strip_prefix(name)
                        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                };
                if counted(token, "repeat=")
                    || counted(token, "similar=")
                    || counted(token, "last=")
                {
                    shown = shown.split_once(' ').unwrap().1;
                } else if token == "text_truncated" {
                    let rest = shown.split_once(' ').unwrap().1;
                    let (bytes, text) = rest.split_once(' ').unwrap();
                    truncated = Some(
                        bytes
                            .strip_prefix("bytes=")
                            .unwrap()
                            .parse::<usize>()
                            .unwrap(),
                    );
                    shown = text;
                    break;
                } else {
                    break;
                }
            }
            let full = &source[start - 1];
            if let Some(bytes) = truncated {
                assert!(full.starts_with(shown), "seed {seed}: line {start} prefix");
                assert!(bytes >= shown.len(), "seed {seed}: line {start} byte count");
                continue;
            }
            assert_eq!(*full, shown, "seed {seed}: line {start}");
            units += 1;
        }
        views += 1;
        assert!(
            block.is_none_or(|(at, end)| at == end + 1),
            "seed {seed}: block header promised lines that were not emitted"
        );
    }
    // A property test that checked nothing would also report success.
    assert!(views >= 35, "only {views} inputs reached the engine");
    assert!(units >= 600, "only {units} labels were resolved");
}

/// What a terminal leaves on screen for one source line: the same rule the
/// engine applies, restated independently so the test does not assert the
/// implementation against itself.
fn terminal_view(line: &str) -> String {
    let body = line.strip_suffix('\n').unwrap_or(line);
    let body = body.strip_suffix('\r').unwrap_or(body);
    let mut out = String::new();
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                while chars
                    .peek()
                    .is_some_and(|c| ('\u{30}'..='\u{3f}').contains(c))
                {
                    chars.next();
                }
                while chars
                    .peek()
                    .is_some_and(|c| ('\u{20}'..='\u{2f}').contains(c))
                {
                    chars.next();
                }
                if chars
                    .peek()
                    .is_some_and(|c| ('\u{40}'..='\u{7e}').contains(c))
                {
                    chars.next();
                }
            }
            Some(']') => {
                chars.next();
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            Some(&c) if ('\u{20}'..='\u{2f}').contains(&c) => {
                while chars
                    .peek()
                    .is_some_and(|c| ('\u{20}'..='\u{2f}').contains(c))
                {
                    chars.next();
                }
                if chars
                    .peek()
                    .is_some_and(|c| ('\u{30}'..='\u{7e}').contains(c))
                {
                    chars.next();
                }
            }
            Some(&c) if ('\u{40}'..='\u{5f}').contains(&c) => {
                chars.next();
            }
            _ => {}
        }
    }
    out.rsplit('\r')
        .find(|segment| !segment.is_empty())
        .unwrap_or("")
        .to_owned()
}

/// The displayed text of a unit line, after its label and metadata tokens.
fn shown_text(line: &str) -> Option<&str> {
    let (label, mut body) = line.split_once(' ')?;
    if !label_numbers(label) {
        return None;
    }
    while let Some((token, rest)) = body.split_once(' ') {
        if ["repeat=", "similar=", "last="]
            .iter()
            .any(|name| token.starts_with(name))
        {
            body = rest;
        } else {
            break;
        }
    }
    Some(body)
}

/// Reading source code is the common case that folding hurt: closing braces
/// and blank lines folded into one far-away `repeat=` unit, so no function
/// could be read whole. Trivial lines stay where they are, and the retained
/// code reads as contiguous blocks.
#[test]
fn a_code_read_keeps_its_braces_and_blank_lines_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/bench/fixtures/rust_large.txt"
    ))
    .unwrap();
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    for line in text.lines().filter(|l| l.contains(" last=")) {
        let shown = shown_text(line).unwrap();
        assert!(
            shown.chars().filter(|c| !c.is_whitespace()).count() > 3,
            "trivial line folded away from its place: {line:?} in:\n{text}"
        );
    }
    assert!(
        text.contains("\n }\n"),
        "no closing brace in a block:\n{text}"
    );
    assert!(text.contains("\n \n"), "no blank line in a block:\n{text}");
    assert_eq!(original(dir.path(), &text), raw.as_bytes());

    // A trivial line between two retained diagnostic lines stays between them.
    let raw = format!(
        "{}error[E0425]: cannot find value `beta`\n   |\n12 |     let total = alpha + beta;\n   |\n{}",
        noise_lines(400),
        noise_lines(400)
    );
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    assert!(
        text.contains(" error[E0425]: cannot find value `beta`\n    |\n 12 |     let total = alpha + beta;\n    |\n"),
        "{text}"
    );
}

/// The most common real shape: one failing test among hundreds of passes.
/// The failure's own detail is the reason the view exists, so it must survive
/// the passes, which fold into a single line.
#[test]
fn one_failure_among_hundreds_of_passes_keeps_its_detail() {
    let dir = tempfile::tempdir().unwrap();
    let mut raw = String::from("> app@1.4.2 test\n> jest --runInBand\n\n");
    for i in 0..240 {
        raw.push_str(&format!("PASS  src/modules/module{i}.test.js\n"));
    }
    raw.push_str(
        "FAIL  src/auth/session.test.js\n  \u{25cf} session \u{203a} refreshes an expiring token\n\n    expect(received).toBe(expected)\n\n    Expected: 1735689600\n    Received: 1735689599\n\n      at Object.<anonymous> (src/auth/session.test.js:42:31)\n",
    );
    for i in 240..480 {
        raw.push_str(&format!("PASS  src/modules/module{i}.test.js\n"));
    }
    raw.push_str("\nTest Suites: 1 failed, 480 passed, 481 total\nTests:       1 failed, 1327 passed, 1328 total\n");
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    for expected in [
        "FAIL  src/auth/session.test.js\n",
        "Expected: 1735689600\n",
        "Received: 1735689599\n",
        "at Object.<anonymous> (src/auth/session.test.js:42:31)\n",
        "Test Suites: 1 failed, 480 passed, 481 total\n",
        "similar=480",
    ] {
        assert!(text.contains(expected), "{expected:?} missing in:\n{text}");
    }
    // Folding the passes is what leaves room for the failure.
    assert!(
        text.matches("PASS  src/modules").count() == 1,
        "passes were not folded:\n{text}"
    );
    assert!(text.len() < 1200, "{} bytes:\n{text}", text.len());
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

/// Shapes agents actually run, beyond the synthetic fixtures: the evidence
/// that decides the outcome has to survive, and the noise around it has to
/// fold.
#[test]
fn real_tool_output_keeps_the_evidence_that_decides_the_outcome() {
    let mut cases: Vec<(&str, String, Vec<&str>)> = Vec::new();

    let mut cargo: String = (0..300)
        .map(|i| format!("   Compiling crate_{i} v0.{i}.1\n"))
        .collect();
    cargo.push_str("warning: unused variable: `ctx`\n --> src/handler.rs:88:9\n  |\n88 |     let ctx = build();\n  |         ^^^ help: prefix with underscore: `_ctx`\n\n");
    cargo.push_str("error[E0599]: no method named `finish` found for struct `Builder`\n   --> src/handler.rs:120:22\n    |\n120 |     let out = builder.finish();\n    |                      ^^^^^^ method not found in `Builder`\n\n");
    cargo.extend((300..420).map(|i| format!("   Compiling crate_{i} v0.{i}.1\n")));
    cargo.push_str(
        "error: could not compile `app` (bin \"app\") due to 1 previous error; 1 warning emitted\n",
    );
    cases.push((
        "cargo build",
        cargo,
        vec![
            "error[E0599]",
            "no method named `finish`",
            "--> src/handler.rs:120:22",
            "could not compile",
            "warning: unused variable",
        ],
    ));

    let mut tsc: Vec<String> = (0..400)
        .map(|i| format!("src/components/Widget{i}.tsx:{i}:5 - checked"))
        .collect();
    tsc.insert(
        200,
        "src/api/client.ts(45,7): error TS2322: Type 'string' is not assignable to type 'number'."
            .into(),
    );
    tsc.push("Found 1 error in src/api/client.ts:45".into());
    cases.push((
        "tsc",
        tsc.join("\n") + "\n",
        vec!["TS2322", "not assignable", "Found 1 error"],
    ));

    let mut pytest: String = (0..400)
        .map(|i| {
            format!(
                "tests/test_mod{i}.py::test_case_{i} PASSED                    [ {:2}%]\n",
                i / 5
            )
        })
        .collect();
    pytest.push_str("tests/test_auth.py::test_refresh FAILED                      [100%]\n\n=================================== FAILURES ===================================\n");
    pytest.push_str(">       assert token.expires_at == 1735689600\nE       assert 1735689599 == 1735689600\n\ntests/test_auth.py:42: AssertionError\n");
    pytest.push_str("FAILED tests/test_auth.py::test_refresh - assert 1735689599 == 1735689600\n======================== 1 failed, 400 passed in 12.4s =========================\n");
    cases.push((
        "pytest -v",
        pytest,
        vec![
            "1735689599 == 1735689600",
            "AssertionError",
            "1 failed, 400 passed",
        ],
    ));

    let mut diff = String::from(
        "diff --git a/src/auth.rs b/src/auth.rs\nindex 1a2b3c4..5d6e7f8 100644\n--- a/src/auth.rs\n+++ b/src/auth.rs\n@@ -40,7 +40,7 @@ impl Session {\n     fn is_expired(&self, now: u64) -> bool {\n-        self.expires_at > now\n+        self.expires_at >= now\n     }\n",
    );
    for f in 0..60 {
        diff.push_str(&format!("diff --git a/src/gen/mod{f}.rs b/src/gen/mod{f}.rs\nindex aaa{f}..bbb{f} 100644\n--- a/src/gen/mod{f}.rs\n+++ b/src/gen/mod{f}.rs\n@@ -1,3 +1,3 @@\n-pub const BUILD: u32 = 1;\n+pub const BUILD: u32 = 2;\n \n"));
    }
    cases.push((
        "git diff",
        diff,
        vec!["+        self.expires_at >= now", "a/src/auth.rs"],
    ));

    for (name, raw, facts) in cases {
        let dir = tempfile::tempdir().unwrap();
        let text = compress(dir.path(), raw.as_bytes(), 4096);
        for fact in &facts {
            assert!(text.contains(fact), "{name}: {fact:?} missing in:\n{text}");
        }
        // Half the budget or better, otherwise the noise was not folded.
        assert!(
            text.len() * 2 <= raw.len(),
            "{name}: {} bytes from {}",
            text.len(),
            raw.len()
        );
        assert_eq!(original(dir.path(), &text), raw.as_bytes(), "{name}");
    }
}

// Relevance anchors.

/// A line naming a term of the task is shown although nothing else ranks it:
/// the view depends on the anchors, the artifact does not.
#[test]
fn anchor_terms_rank_their_lines_into_the_view() {
    let dir = tempfile::tempdir().unwrap();
    let mut raw = distinct_lines(3000);
    let at = raw.match_indices('\n').nth(1499).unwrap().0 + 1;
    raw.insert_str(at, "bcde parse_header unit ready\n");
    let plain = anchored(dir.path(), &raw, None, &[]);
    assert!(!plain.contains("parse_header"), "{plain}");
    let focused = anchored(dir.path(), &raw, None, &["parse_header"]);
    assert!(
        focused.contains("\n1501 bcde parse_header unit ready\n"),
        "{focused}"
    );
    assert_eq!(short_id(&plain), short_id(&focused));
}

/// A line an anchor ranks up is relevant, not a diagnostic: it does not cap
/// the ordinary lines around it. Treated as one, a source read naming a
/// prompt term filled a quarter of its budget (1548 bytes of 4096 for a
/// 4836-byte read) and was compressed where it would otherwise stay exact.
#[test]
fn anchored_lines_do_not_cap_the_ordinary_lines_around_them() {
    let dir = tempfile::tempdir().unwrap();
    let raw: String = distinct_lines(240)
        .lines()
        .enumerate()
        .map(|(i, line)| {
            if i % 60 == 7 {
                format!("    let mode = Mode::Fast; // {line}\n")
            } else {
                format!("    let {line} = compute();\n")
            }
        })
        .collect();
    let plain = anchored(dir.path(), &raw, None, &[]);
    let focused = anchored(dir.path(), &raw, None, &["Mode"]);
    assert!(focused.contains("Mode::Fast"), "{focused}");
    assert!(
        focused.len() * 10 >= plain.len() * 9,
        "{} of {}: {focused}",
        focused.len(),
        plain.len()
    );
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
    let text = anchored(dir.path(), &raw, None, &["tick"]);
    assert!(text.len() <= 4096, "{}", text.len());
    // Shown lines may sit in range blocks: `a-b`, then one per line.
    assert!(text.contains("error: replica 7 lost quorum\n"), "{text}");
    assert!(text.contains("tick a unit ready\n"), "{text}");
    assert!(text.contains("tick cjjj unit ready\n"), "{text}");
    let ticks = text.lines().filter(|l| l.contains("tick")).count();
    let plain = anchored(dir.path(), &raw, None, &[]);
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
    let plain = anchored(dir.path(), &source, Some(Profile::FileRead), &[]);
    assert!(!plain.contains("resolve_prefix"), "{plain}");
    let focused = anchored(
        dir.path(),
        &source,
        Some(Profile::FileRead),
        &["resolve_prefix"],
    );
    assert!(
        focused.contains("fn resolve_prefix(text: &str) -> bool {"),
        "{focused}"
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
    let plain = anchored(dir.path(), &raw, Some(Profile::Search), &["let "]);
    assert!(!plain.contains("resolve_prefix"), "{plain}");
    let focused = anchored(
        dir.path(),
        &raw,
        Some(Profile::Search),
        &["let ", "resolve_prefix"],
    );
    assert!(focused.contains("resolve_prefix"), "{focused}");
}

fn cli(dir: &std::path::Path) -> assert_cmd::Command {
    let mut command = assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("scopelet"));
    command
        .env("SCOPELET_CONFIG_DIR", dir.join("config"))
        .env("SCOPELET_CACHE_DIR", dir.join("cache"));
    command
}

fn hook(dir: &std::path::Path, agent: &str, event: Value) -> Value {
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
    let tool = |command: &str| json!({"hook_event_name":"PostToolUse","session_id":"a1","tool_name":"Bash","tool_input":{"command":command},"tool_response":{"stdout":raw,"stderr":"","interrupted":false,"isImage":false}});
    let view = |result: Value| -> String {
        result["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    // A different session sees no prompt anchors yet.
    let before = view(hook(dir.path(), "claude", tool("npm run build")));
    assert!(!before.contains("parse_header"), "{before}");
    let prompt = json!({"hook_event_name":"UserPromptSubmit","session_id":"a2","prompt":"Why is `parse_header` slow?"});
    hook(dir.path(), "claude", prompt);
    let mut event = tool("npm run build");
    event["session_id"] = json!("a2");
    let after = view(hook(dir.path(), "claude", event));
    assert!(after.contains("bcde parse_header unit ready"), "{after}");
    // The command's search pattern anchors on its own.
    let mut search = tool("rg -n parse_header");
    search["session_id"] = json!("a3");
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

// Test runs.

fn test_view(dir: &std::path::Path, raw: &str) -> String {
    let output = compress::automatic_decision(
        raw.as_bytes(),
        Some(dir.to_path_buf()),
        Some(Profile::Tests),
        None,
    )
    .map(|(view, _)| view)
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

/// A cargo run over several test binaries carries its totals below the
/// header (after the failure line), summed from every `test result:` line and
/// saying so: without it, live sessions trusted their own partial sum of the
/// results shown. Those lines are shown before ordinary ones and never fold
/// by their numbers, so the results in view add up to the totals. One
/// binary's result is its own total.
#[test]
fn cargo_totals_sum_every_test_binary() {
    let dir = tempfile::tempdir().unwrap();
    // Binary k runs 40 + k tests, so no two result lines are identical.
    let binary = |k: usize, failed: usize| {
        let mut out = format!(
            "     Running tests/t{k}.rs (target/debug/deps/t{k}-0123abcd)\n\nrunning {} tests\n{}",
            40 + k,
            passes(40 + k - failed)
        );
        if failed > 0 {
            out.push_str(&format!(
                "test t{k}::breaks ... FAILED\n\nfailures:\n\n---- t{k}::breaks stdout ----\nthread 't{k}::breaks' panicked at tests/t{k}.rs:9:5:\nboom\n\nfailures:\n    t{k}::breaks\n\n"
            ));
        }
        out.push_str(&format!(
            "test result: {}. {} passed; {failed} failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s\n\n",
            if failed > 0 { "FAILED" } else { "ok" },
            40 + k - failed
        ));
        out
    };
    let raw: String = (0..17).map(|k| binary(k, 0)).collect();
    let text = test_view(dir.path(), &raw);
    assert_eq!(
        text.lines().nth(1),
        Some(
            "totals runs=17 passed=816 failed=0 ignored=17 (every test result line, omitted ones included)"
        ),
        "{text}"
    );
    assert!(text.contains(" omitted="), "{text}");
    assert_eq!(text.matches("test result: ok. ").count(), 17, "{text}");
    assert!(
        !text
            .lines()
            .any(|l| l.contains(" similar=") && l.contains("test result:")),
        "{text}"
    );

    let raw: String = (0..17).map(|k| binary(k, usize::from(k == 9))).collect();
    let text = test_view(dir.path(), &raw);
    assert_eq!(
        action_line(&text),
        Some("failed=1: t9::breaks (tests/t9.rs:9)"),
        "{text}"
    );
    assert_eq!(
        text.lines().nth(2),
        Some(
            "totals runs=17 passed=815 failed=1 ignored=17 (every test result line, omitted ones included)"
        ),
        "{text}"
    );
    assert!(
        text.contains("test result: FAILED. 48 passed; 1 failed;"),
        "{text}"
    );
    assert!(text.contains("panicked at tests/t9.rs:9:5"), "{text}");

    let raw = binary(0, 0) + &passes(200);
    let text = test_view(dir.path(), &raw);
    assert!(!text.contains("totals "), "{text}");
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
        let text = test_view(dir.path(), raw);
        assert_eq!(action_line(&text), Some(expected), "{text}");
        assert!(text.len() <= 4096, "{}", text.len());
        assert_eq!(original(dir.path(), &text), raw.as_bytes());
    }
}

/// Five failures are named and the rest counted; output no runner format
/// matches gets no line rather than a guess, and only test views get one.
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
    let text = test_view(dir.path(), &raw);
    let line = action_line(&text).unwrap();
    assert!(
        line.starts_with("failed=8: TestCase0 (case_test.go:10), "),
        "{line}"
    );
    assert!(line.ends_with("TestCase4 (case_test.go:14), +3"), "{line}");

    let unknown = format!("{}error: something odd happened\n", passes(600));
    let text = test_view(dir.path(), &unknown);
    assert_eq!(action_line(&text), None, "{text}");

    let search = compress::automatic_decision(
        raw.as_bytes(),
        Some(dir.path().into()),
        Some(Profile::Search),
        None,
    )
    .map(|(view, _)| view)
    .unwrap();
    assert!(!String::from_utf8_lossy(&search).contains("failed=8"));
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
    let text = test_view(dir.path(), &raw);
    assert_eq!(
        action_line(&text),
        Some("failed=1: TestRotate (/work/gmod/calc.go:7)"),
        "{text}"
    );
}

/// Audit finding: a failure line too long for a small budget is left out
/// instead of making the view fail back to the raw output.
#[test]
fn failures_never_starve_a_small_budget() {
    let dir = tempfile::tempdir().unwrap();
    let long = |k: usize| {
        format!(
            "tests::{}_{}",
            "very_long_module_name_é".repeat(5),
            letters(k)
        )
    };
    let mut raw = format!("running 600 tests\n{}failures:\n\n", passes(600));
    for k in 0..6 {
        raw.push_str(&format!(
            "---- {0} stdout ----\nthread '{0}' panicked at src/lib.rs:{1}:5:\nboom\n\n",
            long(k),
            10 + k
        ));
    }
    raw.push_str("test result: FAILED. 594 passed; 6 failed\n");
    let output = compress::automatic_decision(
        raw.as_bytes(),
        Some(dir.path().into()),
        Some(Profile::Tests),
        Some(1024),
    )
    .map(|(view, _)| view)
    .unwrap();
    assert!(output.len() <= 1024, "{} bytes", output.len());
    assert!(output.starts_with(b"[scopelet v5 "));
    let text = String::from_utf8(output.into_owned()).unwrap();
    assert_eq!(action_line(&text), None, "{text}");
    // At the profile's own budget the line fits and is shown.
    assert!(action_line(&test_view(dir.path(), &raw)).is_some());
}

// Searches.

/// `grep -n` output: `path:N:text` lines, each with a distinct word so no
/// template folds them.
fn grep_output(files: &[(&str, usize)]) -> String {
    let words: Vec<String> = (0..2000).map(letters).collect();
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
    let text = anchored(dir.path(), &small, Some(Profile::Search), &[]);
    assert!(first_line(&text).ends_with(" complete]\n"), "{text}");
    assert!(
        text.contains("\n1-100 file=src/alpha.rs\n 10:    let a = 1;\n"),
        "{text}"
    );
    assert!(text.contains(" 703:    let jj = 1;\n"), "{text}");
    assert!(
        text.contains("\n101-140 file=src/beta.rs\n 10:    let baa = 1;\n"),
        "{text}"
    );
    assert!(!text.contains("more="), "{text}");
    assert!(!text.contains("input:"), "{text}");
    assert_eq!(original(dir.path(), &text), small.as_bytes());

    let names: Vec<String> = (0..30).map(|f| format!("src/m{}.rs", letters(f))).collect();
    let files: Vec<(&str, usize)> = names.iter().map(|n| (n.as_str(), 40)).collect();
    let raw = grep_output(&files);
    let words: Vec<String> = (0..2000).map(letters).collect();
    // Match k of file f declares words[40 f + k] on line 10 + 7k.
    let line = |f: usize, k: usize| format!(" {}:    let {} = 1;\n", 10 + k * 7, words[40 * f + k]);
    let view = |anchors: &[&str]| anchored(dir.path(), &raw, Some(Profile::Search), anchors);
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
}

/// Context lines (`path-N-text`) join their file's block; a context line
/// whose run holds no shown match is not shown either.
#[test]
fn search_context_lines_stay_with_their_matches() {
    let dir = tempfile::tempdir().unwrap();
    let words: Vec<String> = (0..3000).map(letters).collect();
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
    let text = anchored(dir.path(), &raw, Some(Profile::Search), &[]);
    assert!(
        text.contains("\n1-3 file=src/a.rs\n 9-    // baaa before\n 10:    call_caaa();\n 11-    // bfaa after\n"),
        "{text}"
    );
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

// Diffs.

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

/// A unified diff is read file by file: every file and hunk header keeps its
/// place, changed lines outrank context, context never folds away from its
/// hunk, and code that mentions `error` is a change, not a diagnostic.
#[test]
fn unified_diffs_keep_file_headers_and_changes_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let mut raw = String::new();
    for f in 0..12 {
        raw.push_str(&format!(
            "diff --git a/src/m{f}.rs b/src/m{f}.rs\nindex 1a2b3c{f}..4d5e6f{f} 100644\n--- a/src/m{f}.rs\n+++ b/src/m{f}.rs\n@@ -{0},7 +{0},7 @@ fn run() {{\n     let a = 1;\n     let b = 2;\n-    return Err(error_{f});\n+    return Ok(value_{f});\n     }}\n }}\n",
            10 + f
        ));
    }
    for i in 0..300 {
        raw.push_str(&format!(" unchanged context line {i}\n"));
    }
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    for f in 0..12 {
        assert!(
            text.contains(&format!("diff --git a/src/m{f}.rs b/src/m{f}.rs")),
            "header {f} missing in:\n{text}"
        );
        assert!(
            text.contains(&format!("+    return Ok(value_{f});")),
            "{text}"
        );
    }
    // Context stays beside its change instead of folding into a distant copy.
    assert!(!text.contains("repeat=12 last="), "{text}");
    assert!(
        text.contains("-    return Err(error_0);\n +    return Ok(value_0);\n      }\n"),
        "{text}"
    );
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

/// Git's default three lines of context put a hunk header four lines above
/// its first change: the header still locates the changes shown. A tier is
/// filled from both ends, so where the budget runs out one hunk may be cut
/// between its header and its change.
#[test]
fn hunk_headers_stay_with_their_changes_at_the_default_context() {
    let dir = tempfile::tempdir().unwrap();
    let mut raw = String::new();
    for f in 0..40 {
        raw.push_str(&format!(
            "diff --git a/src/m{f}.py b/src/m{f}.py\nindex 1111111..2222222 100644\n--- a/src/m{f}.py\n+++ b/src/m{f}.py\n@@ -{0},7 +{0},8 @@ def handler_{f}():\n     x = load()\n     y = parse(x)\n     z = check(y)\n-    return old_{f}(z)\n+    value = new_{f}(z)\n+    return value\n     # end\n     pass\n     done()\n",
            10 * f + 5
        ));
    }
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    let shown: Vec<usize> = (0..40)
        .filter(|f| text.contains(&format!("+    value = new_{f}(z)\n")))
        .collect();
    assert!(
        shown.len() >= 5,
        "{} changes shown in:\n{text}",
        shown.len()
    );
    let headless: Vec<_> = shown
        .iter()
        .filter(|f| !text.contains(&format!("@@ def handler_{f}():\n")))
        .collect();
    assert!(
        headless.len() <= 1,
        "{headless:?} without header in:\n{text}"
    );
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

/// Commit messages fold only with identical text, like changes: messages
/// that differ by a number are distinct evidence, not a template.
#[test]
fn commit_messages_differing_by_numbers_stay_distinct() {
    let dir = tempfile::tempdir().unwrap();
    let mut raw = String::new();
    for c in 0..40 {
        raw.push_str(&format!(
            "commit {c:040x}\nAuthor: A <a@example.com>\nDate:   Mon Jan 1 10:00:00 2024 +0000\n\n    Fix parser case {c}\n\ndiff --git a/src/m{c}.rs b/src/m{c}.rs\nindex 1111111..2222222 100644\n--- a/src/m{c}.rs\n+++ b/src/m{c}.rs\n@@ -1,2 +1,2 @@\n-old {c}\n+new {c}\n"
        ));
    }
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    let messages = text.matches("    Fix parser case ").count();
    assert!(messages >= 5, "{messages} messages in:\n{text}");
    assert!(
        !text
            .lines()
            .any(|l| l.contains("similar=") && l.contains("Fix parser case")),
        "{text}"
    );
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

/// A lockfile or minified file's diff becomes one counted line: its changes
/// lose their protection and the budget goes to the real files.
#[test]
fn lockfile_and_generated_diffs_become_one_counted_line() {
    let dir = tempfile::tempdir().unwrap();
    let words: Vec<String> = (0..400).map(letters).collect();
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
    let text = anchored(dir.path(), &raw, Some(Profile::Git), &[]);
    assert!(
        text.contains("\n10-314 file=Cargo.lock changed=+150/-150 omitted\n"),
        "{text}"
    );
    assert!(
        text.contains("\n315-321 file=web/dist/app.min.js changed=+1/-1 omitted\n"),
        "{text}"
    );
    assert!(!text.contains("version = \"0."), "{text}");
    assert!(!text.contains("var a="), "{text}");
    assert!(text.contains("+        self.expires_at >= now"), "{text}");
    assert!(text.contains("+        gross - self.discount"), "{text}");
    assert!(text.starts_with("[scopelet v5 "), "{text}");
    assert!(
        text.contains(" omitted="),
        "the hidden lines are counted: {text}"
    );
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

/// When the real files do not all fit, the densest changes come first: a
/// file of nothing but changes between two files whose changes sit among
/// context is no longer the one left out.
#[test]
fn lockfile_free_diffs_spend_the_budget_on_dense_files_first() {
    let dir = tempfile::tempdir().unwrap();
    let words: Vec<String> = (0..5000).map(letters).collect();
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
    let text = anchored(dir.path(), &raw, Some(Profile::Git), &[]);
    let dense_shown = (0..100)
        .filter(|i| text.contains(&format!("+    call_{}();", words[4000 + i])))
        .count();
    assert_eq!(dense_shown, 100, "{text}");
    assert!(text.contains("diff --git a/src/a.rs"), "{text}");
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

// Listings.

/// A directory of more than eight entries in a `find` listing is summarized
/// by count and extension, with its first and last entries; a small one is
/// listed whole. A listing that fits is never folded.
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
    let text = anchored(dir.path(), &raw, Some(Profile::Search), &[]);
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
    assert!(!text.contains("input:"), "{text}");
    let omitted: usize = first_line(&text)
        .split("omitted=")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(omitted, 400);
    assert_eq!(original(dir.path(), &text), raw.as_bytes());

    // A listing that fits the budget is not folded: no name is hidden, even
    // under a build or dependency directory.
    let mut fits: String = raw.lines().take(150).map(|l| format!("{l}\n")).collect();
    for i in 0..20 {
        fits.push_str(&format!("./node_modules/pkg-{}/index.js\n", letters(i)));
    }
    let text = anchored(dir.path(), &fits, Some(Profile::Search), &[]);
    assert!(!text.contains(" files="), "{text}");
    assert!(text.contains("./node_modules/pkg-bj/index.js"), "{text}");
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
    let text = anchored(dir.path(), &ls, Some(Profile::Search), &[]);
    assert!(text.contains("./src/ files=800 ext=py:800\n"), "{text}");
    assert!(text.contains("./src/static:"), "{text}");
    assert!(text.contains("app.css"), "{text}");
    assert_eq!(original(dir.path(), &text), ls.as_bytes());

    let mut tree = String::from(".\n├── Cargo.toml\n└── src\n    ├── main.rs\n    └── handlers\n");
    for i in 0..500 {
        tree.push_str(&format!("        ├── handler_{}.rs\n", letters(i)));
    }
    tree.push_str("        └── mod.rs\n\n3 directories, 504 files\n");
    let text = anchored(dir.path(), &tree, Some(Profile::Search), &[]);
    assert!(
        text.contains("src/handlers/ files=501 ext=rs:501\n"),
        "{text}"
    );
    assert!(text.contains("└── mod.rs"), "{text}");
    assert!(text.contains("3 directories, 504 files"), "{text}");
    assert_eq!(original(dir.path(), &text), tree.as_bytes());
}

// Installs.

/// An install view keeps its warnings, errors and closing summary, never
/// the progress lines between them.
#[test]
fn install_views_keep_only_what_decides_the_outcome() {
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
    let output = compress::automatic_decision(
        raw.as_bytes(),
        Some(dir.path().into()),
        Some(Profile::Install),
        None,
    )
    .map(|(view, _)| view)
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

/// Found on a real `pip install`: the line saying what was installed is the
/// outcome even when warnings follow it.
#[test]
fn install_views_keep_the_installed_summary() {
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
    let output = compress::automatic_decision(
        raw.as_bytes(),
        Some(dir.path().into()),
        Some(Profile::Install),
        None,
    )
    .map(|(view, _)| view)
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

// Tables.

/// One CSV cell read back as JSON: a quoted cell is a JSON string, a bare
/// one a JSON scalar when it parses as one, else a string.
fn cell(text: &str) -> Value {
    if text.starts_with('"') {
        return serde_json::from_str(text).unwrap();
    }
    serde_json::from_str::<Value>(text)
        .ok()
        .filter(|v| !v.is_string())
        .unwrap_or_else(|| Value::String(text.to_owned()))
}

/// The cells of a CSV line, split on commas outside JSON strings.
fn split(line: &str) -> Vec<&str> {
    let (mut parts, mut start, mut quoted, mut escaped) = (Vec::new(), 0, false, false);
    for (i, c) in line.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ',' if !quoted => {
                parts.push(&line[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&line[start..]);
    parts
}

fn cells(line: &str) -> Vec<Value> {
    split(line).into_iter().map(cell).collect()
}

/// A table as a view writes it: `LABEL COLUMNS rows=N`, `rows=N/TOTAL` and
/// maybe `indices=RANGES` when only some rows are shown, then `constant` and
/// `omitted_columns` lines, then one CSV row per record.
struct Table {
    columns: Vec<String>,
    constant: serde_json::Map<String, Value>,
    omitted: Vec<String>,
    rows: Vec<Vec<Value>>,
    total: Option<usize>,
    indices: Option<Vec<usize>>,
}

impl Table {
    fn read(text: &str, label: &str) -> Self {
        let mut lines = text
            .lines()
            .skip_while(|l| !l.starts_with(&format!("{label} ")));
        let head = lines
            .next()
            .unwrap_or_else(|| panic!("no {label:?} table in {text}"));
        let (columns, counts) = head[label.len() + 1..]
            .rsplit_once(" rows=")
            .unwrap_or_else(|| panic!("not a CSV table: {head}"));
        let (counts, indices) = match counts.split_once(" indices=") {
            Some((counts, ranges)) => (
                counts,
                Some(
                    ranges
                        .split(',')
                        .flat_map(|part| match part.split_once('-') {
                            Some((a, b)) => (a.parse().unwrap()..=b.parse().unwrap()).collect(),
                            None => vec![part.parse().unwrap()],
                        })
                        .collect(),
                ),
            ),
            None => (counts, None),
        };
        let (shown, total) = match counts.split_once('/') {
            Some((shown, total)) => (shown, Some(total.parse().unwrap())),
            None => (counts, None),
        };
        let mut table = Self {
            columns: cells(columns)
                .into_iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect(),
            constant: serde_json::Map::new(),
            omitted: Vec::new(),
            rows: Vec::new(),
            total,
            indices,
        };
        for line in lines {
            if let Some(pairs) = line.strip_prefix("constant ") {
                for pair in split(pairs) {
                    let (key, value) = pair.split_once('=').unwrap();
                    table
                        .constant
                        .insert(cell(key).as_str().unwrap().to_owned(), cell(value));
                }
            } else if let Some(names) = line.strip_prefix("omitted_columns ") {
                table.omitted = cells(names)
                    .into_iter()
                    .map(|v| v.as_str().unwrap().to_owned())
                    .collect();
            } else {
                let row = cells(line);
                assert_eq!(row.len(), table.columns.len(), "{line}");
                table.rows.push(row);
            }
        }
        assert_eq!(table.rows.len(), shown.parse::<usize>().unwrap(), "{text}");
        table
    }
    /// Every row as the object it was: its columns, then its constants.
    fn objects(&self) -> Vec<Value> {
        self.rows
            .iter()
            .map(|row| {
                let mut object: serde_json::Map<String, Value> = self
                    .columns
                    .iter()
                    .cloned()
                    .zip(row.iter().cloned())
                    .collect();
                object.extend(self.constant.clone());
                Value::Object(object)
            })
            .collect()
    }
}

/// The JSON after a `NAME ` prefix on a view line.
fn json_after(text: &str, prefix: &str) -> Value {
    let line = text
        .lines()
        .find_map(|l| l.strip_prefix(prefix))
        .unwrap_or_else(|| panic!("no {prefix:?} line in {text}"));
    serde_json::from_str(line).unwrap()
}

/// A complete table has no positional footer: the header says the display
/// is complete, and constant columns are named once.
#[test]
fn a_complete_table_carries_only_the_header() {
    let dir = tempfile::tempdir().unwrap();
    let records: Vec<Value> = (0..60)
        .map(|i| json!({"identifier": i, "status": "ok", "region": "eu-west"}))
        .collect();
    let raw = records
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    assert!(first_line(&text).ends_with(" complete]\n"), "{text}");
    assert_eq!(text.matches("[scopelet ").count(), 1, "{text}");
    assert_eq!(
        text.lines().nth(1),
        Some("table identifier rows=60"),
        "{text}"
    );
    let table = Table::read(&text, "table");
    assert_eq!(table.objects(), records);
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

/// A top-level array keeps its table and names its constant columns once.
#[test]
fn tables_name_constant_columns_once() {
    let dir = tempfile::tempdir().unwrap();
    let rows: Vec<Value> = (0..80)
        .map(|i| json!({"id": i, "region": "eu-west-3", "status": "ok", "owner": "platform-team"}))
        .collect();
    let raw = rows
        .iter()
        .map(|r| r.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    let table = Table::read(&text, "table");
    assert_eq!(table.columns, ["id"]);
    assert_eq!(table.objects(), rows);
    assert!(table.indices.is_none(), "{text}");
    assert_eq!(text.matches("eu-west-3").count(), 1, "{text}");
}

/// Tables of scalars are a header and CSV rows; every shown cell reads back
/// as the very value it holds, digits, types and edge spaces included.
#[test]
fn tables_read_back_cell_for_cell() {
    let dir = tempfile::tempdir().unwrap();
    let tricky = [
        "12",
        "1e5",
        "true",
        "null",
        "",
        " edge",
        "a,b",
        "say \"hi\"",
        "k=v",
        "[x]",
        "tab\tin",
        "plain words",
        "é unicode",
    ];
    let rows: Vec<Value> = (0..400)
        .map(|i| {
            json!({
                "id": i,
                "text": tricky[i % tricky.len()],
                "ratio": if i % 2 == 0 { json!(0.10) } else { json!(-3) },
                "flag": i % 3 == 0,
                "exact": 900719925474099312345u128,
                "region": "eu-west",
            })
        })
        .collect();
    let raw = rows
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    assert!(first_line(&text).contains(" omitted="), "{text}");
    assert_eq!(
        text.lines().nth(2),
        Some("constant exact=900719925474099312345,region=eu-west")
    );
    let table = Table::read(&text, "table");
    assert_eq!(table.total, Some(400));
    let indices = table.indices.clone().unwrap();
    assert_eq!(indices.len(), table.rows.len());
    for (object, &i) in table.objects().iter().zip(&indices) {
        assert_eq!(object, &rows[i], "row {i}");
    }
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

/// A table with a nested cell stays a JSON table: CSV would split it.
#[test]
fn nested_cells_keep_the_json_table() {
    let dir = tempfile::tempdir().unwrap();
    let raw = (0..300)
        .map(|i| json!({"id": i, "tags": ["a", "b"], "note": format!("n{i}")}).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    assert!(
        text.lines().nth(1).unwrap().starts_with("table {"),
        "{text}"
    );
}

/// An object whose array of same-keyed objects dominates it is shown as its
/// other fields, then that array as a table whose constant columns appear
/// once; every value survives exactly, nulls and long decimals included.
#[test]
fn json_envelope_with_a_dominant_array_becomes_a_table() {
    let dir = tempfile::tempdir().unwrap();
    let items: Vec<Value> = (0..30)
        .map(|i| {
            json!({
                "apiVersion": "v1",
                "kind": "Pod",
                "name": format!("api-{}", letters(i)),
                "phase": if i == 17 { "CrashLoopBackOff" } else { "Running" },
                "restarts": i % 4,
                "quota": null,
            })
        })
        .collect();
    let exact: Value = serde_json::from_str("900719925474099312345.10").unwrap();
    let document = json!({
        "apiVersion": "v1",
        "kind": "List",
        "items": items,
        "metadata": {"resourceVersion": exact},
    });
    let raw = serde_json::to_string_pretty(&document).unwrap();
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    assert!(first_line(&text).ends_with(" complete]\n"), "{text}");
    let envelope = json_after(&text, "envelope ");
    assert_eq!(
        envelope,
        json!({"apiVersion": "v1", "kind": "List", "metadata": {"resourceVersion": exact}})
    );
    assert!(text.contains("900719925474099312345.10"), "{text}");
    let table = Table::read(&text, "table=/items");
    assert_eq!(
        Value::Object(table.constant.clone()),
        json!({"apiVersion": "v1", "kind": "Pod", "quota": null})
    );
    assert_eq!(table.objects(), items);
    assert!(text.contains("CrashLoopBackOff"), "{text}");
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

/// No field is invented: items with different keys are not a table, and an
/// array too large to show whole keeps the line view.
#[test]
fn json_items_that_differ_or_overflow_keep_the_line_view() {
    let dir = tempfile::tempdir().unwrap();
    let mut items: Vec<Value> = (0..30)
        .map(|i| json!({"name": format!("job-{}", letters(i)), "state": "done"}))
        .collect();
    items[9] = json!({"name": "job-j", "state": "failed", "error": "OOMKilled"});
    let raw = serde_json::to_string_pretty(&json!({"items": items, "count": 30})).unwrap();
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    assert!(!text.contains("table="), "{text}");
    assert!(text.contains("OOMKilled"), "{text}");

    let big: Vec<Value> = (0..400)
        .map(|i| json!({"name": format!("job-{}", letters(i)), "detail": letters(i * 7919)}))
        .collect();
    let raw = serde_json::to_string_pretty(&json!({"items": big})).unwrap();
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    assert!(!text.contains("table="), "{text}");
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

/// Found on real `gh api` responses (165 to 265 KB on one line): their
/// objects' short fields become a table, nested and long fields are named,
/// and the rows that do not fit are counted.
#[test]
fn json_minified_api_responses_show_a_projected_table() {
    let dir = tempfile::tempdir().unwrap();
    let mut pulls: Vec<Value> = (0..40)
        .map(|i| {
            json!({
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
    pulls[36]["title"] = json!("error: release build failed");
    let raw = serde_json::to_string(&pulls).unwrap();
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    let table = Table::read(&text, "table");
    // The row carrying a diagnostic comes first, wherever it is.
    assert!(text.contains("error: release build failed"), "{text}");
    assert_eq!(table.columns, ["number", "state", "title"]);
    assert_eq!(
        Value::Object(table.constant.clone()),
        json!({"locked": false})
    );
    assert_eq!(table.omitted, ["body", "labels", "user"]);
    assert_eq!(table.total, Some(40));
    let rows = &table.rows;
    assert_eq!(
        rows[0],
        [json!(3500), json!("open"), json!("printer: fix case a")]
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
    let search = json!({"total_count": 251, "incomplete_results": false, "items": pulls});
    let raw = search.to_string();
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    assert_eq!(
        json_after(&text, "envelope "),
        json!({"total_count": 251, "incomplete_results": false})
    );
    assert!(
        !Table::read(&text, "table=/items").rows.is_empty(),
        "{text}"
    );
}

/// Found on real `gh api` responses: one link column is kept and the other
/// links are named, so the rows that matter fit.
#[test]
fn json_projections_keep_one_link_column() {
    let dir = tempfile::tempdir().unwrap();
    let issues: Vec<Value> = (0..30)
        .map(|i| {
            let base = format!("https://api.github.com/repos/acme/app/issues/{i}");
            json!({
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
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    let table = Table::read(&text, "table");
    assert_eq!(table.columns, ["html_url", "number", "title"]);
    assert_eq!(table.omitted, ["comments_url", "events_url", "url", "user"]);
    assert_eq!(table.rows.len(), 30, "{text}");
}

/// Audit finding: a field that only some objects carry, and an envelope's
/// nested field, are named rather than silently left out, and the header is
/// not complete.
#[test]
fn json_projections_name_every_field_they_leave_out() {
    let dir = tempfile::tempdir().unwrap();
    let mut rows: Vec<Value> = (0..40)
        .map(|i| json!({"id": i, "name": format!("n{}", letters(i)), "blob": "z".repeat(1200)}))
        .collect();
    rows[0] = json!({"id": 0, "name": "alpha", "blob": "z".repeat(1200)});
    rows[1]["secret_payload"] = json!("X".repeat(1500));
    let raw = serde_json::to_string(&rows).unwrap();
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    let table = Table::read(&text, "table");
    assert!(
        table.omitted.contains(&"secret_payload".to_owned()),
        "{text}"
    );
    assert!(table.omitted.contains(&"blob".to_owned()), "{text}");
    assert!(!first_line(&text).ends_with(" complete]\n"), "{text}");

    let search = json!({"total_count": 40, "meta": {"page": 1}, "items": rows});
    let raw = search.to_string();
    let text = compress(dir.path(), raw.as_bytes(), 4096);
    let envelope = text.lines().find(|l| l.starts_with("envelope ")).unwrap();
    assert!(
        envelope.ends_with(r#" omitted_fields=["meta"]"#),
        "{envelope}"
    );
    assert!(envelope.contains(r#""total_count":40"#), "{envelope}");
}
