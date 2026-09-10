//! Compact-v3 presentation contracts: cleaned display copies, cut oversized
//! lines, folded repetitions and diagnostic-first selection, with the original
//! bytes always recoverable from the store.
use scopelet::{
    compress::{self, Version},
    model::Dataset,
    store::Store,
};

fn compress_v3(dir: &std::path::Path, raw: &[u8], budget: usize) -> String {
    let output =
        compress::automatic_lazy(raw, Some(dir.to_path_buf()), budget, Version::V3).unwrap();
    assert!(output.len() <= budget, "{} bytes over budget", output.len());
    String::from_utf8(output.into_owned()).unwrap()
}

fn original(dir: &std::path::Path, text: &str) -> Vec<u8> {
    let store = Store::open(Some(dir.to_path_buf())).unwrap();
    let artifact = text
        .split_whitespace()
        .find(|s| s.starts_with("artifact:"))
        .unwrap();
    let data: Dataset = serde_json::from_slice(&store.get(artifact).unwrap()).unwrap();
    store.get(&data.snapshots[0].blob).unwrap()
}

#[test]
fn colored_diagnostics_are_displayed_clean_and_recovered_with_their_codes() {
    let dir = tempfile::tempdir().unwrap();
    let raw = format!(
        "{}\x1b[31mFAILED\x1b[0m tests::alpha\n{}",
        "test ok\n".repeat(1500),
        "test ok\n".repeat(1500)
    );
    let text = compress_v3(dir.path(), raw.as_bytes(), 4096);
    assert!(text.starts_with("[scopelet compact-v3 artifact:"));
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
    let text = compress_v3(dir.path(), raw.as_bytes(), 4096);
    assert!(text.contains("input:1002 100% done\n"), "{text}");
    assert!(!text.contains("10%"), "{text}");
    assert!(
        text.contains("input:2-1001 repeat=1000 building\n"),
        "{text}"
    );
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

#[test]
fn oversized_lines_are_cut_behind_a_marker_and_recovered_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let giant = "é".repeat(6000);
    let text = compress_v3(dir.path(), giant.as_bytes(), 4096);
    assert!(
        text.contains("input:1 text_truncated bytes=12000 éé"),
        "{text}"
    );
    assert!(text.len() * 100 <= giant.len() * 15, "{} bytes", text.len());
    assert_eq!(original(dir.path(), &text), giant.as_bytes());

    let raw = format!(
        "{}{}\nerror: after the giant line\nend\n",
        "ok\n".repeat(200),
        "x".repeat(20000)
    );
    let text = compress_v3(dir.path(), raw.as_bytes(), 4096);
    assert!(
        text.contains("input:201 text_truncated bytes=20000 xxx"),
        "{text}"
    );
    assert!(
        text.contains("input:202 error: after the giant line\n"),
        "{text}"
    );
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

#[test]
fn oversized_json_records_are_never_split() {
    let dir = tempfile::tempdir().unwrap();
    let raw = serde_json::json!({"value":"x".repeat(50000)}).to_string();
    let output =
        compress::automatic_lazy(raw.as_bytes(), Some(dir.path().into()), 4096, Version::V3)
            .unwrap();
    assert_eq!(output.as_ref(), raw.as_bytes());
}

#[test]
fn dispersed_repetitions_fold_into_their_first_occurrence() {
    let dir = tempfile::tempdir().unwrap();
    let raw = format!(
        "start\n{}fatal error: rare root cause\nend\n",
        "error: retry failed\nworking\n".repeat(1000)
    );
    let text = compress_v3(dir.path(), raw.as_bytes(), 4096);
    for expected in [
        "input:1 start\n",
        "input:2 repeat=1000 last=2000 error: retry failed\n",
        "input:3 repeat=1000 last=2001 working\n",
        "input:2002 fatal error: rare root cause\n",
        "input:2003 end\n",
        "[scopelet display_complete=true omitted_units=0;",
    ] {
        assert!(text.contains(expected), "{expected:?} missing in:\n{text}");
    }
    assert!(text.len() < 600, "{} bytes:\n{text}", text.len());
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
    let text = compress_v3(dir.path(), raw.as_bytes(), 4096);
    assert!(
        text.contains("input:1 similar=1000 last=1001 progress 0000: unchanged "),
        "{text}"
    );
    assert!(
        text.contains("input:501 receipt audit-7139 amount=47 EUR padding"),
        "{text}"
    );
    assert!(text.contains("omitted_units=0"), "{text}");
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
}

#[test]
fn crowded_distinct_diagnostics_keep_first_and_final_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let middle = (0..1000)
        .map(|i| format!("error: case {i} failed\n"))
        .collect::<String>();
    let raw = format!("running 1000 tests\n{middle}final status: 1000 failed; exit=7\n");
    let text = compress_v3(dir.path(), raw.as_bytes(), 1024);
    assert!(text.contains("running 1000 tests\n"), "{text}");
    assert!(text.contains(" error: case 0 failed\n"), "{text}");
    assert!(text.contains(" error: case 999 failed\n"), "{text}");
    assert!(
        text.contains("final status: 1000 failed; exit=7\n"),
        "{text}"
    );
    assert!(
        text.contains("display_complete=false omitted_units="),
        "{text}"
    );
}

fn noise_lines(count: usize) -> String {
    (0..count)
        .map(|i| format!("compiling unit {i}\n"))
        .collect()
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
    ];
    let mut raw = String::new();
    for fact in facts {
        raw.push_str(&noise_lines(400));
        raw.push_str(fact);
        raw.push('\n');
    }
    raw.push_str(&noise_lines(400));
    let text = compress_v3(dir.path(), raw.as_bytes(), 4096);
    for fact in facts {
        assert!(text.contains(fact), "{fact:?} missing in:\n{text}");
    }
    assert_eq!(original(dir.path(), &text), raw.as_bytes());
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
    let text = compress_v3(dir.path(), raw.as_bytes(), 1024);
    let errors_shown = text.matches("error: cannot find").count();
    let warnings_shown = text.matches("warning: unused").count();
    assert!(errors_shown >= 5, "{errors_shown} errors in:\n{text}");
    assert!(warnings_shown <= 1, "{warnings_shown} warnings in:\n{text}");
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
    let text = compress_v3(dir.path(), raw.as_bytes(), 1024);
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
    let text = compress_v3(dir.path(), raw.as_bytes(), 4096);
    let lines: Vec<&str> = text.lines().collect();
    let header = lines
        .iter()
        .position(|l| l.starts_with("input:501-"))
        .unwrap_or_else(|| panic!("no range block in:\n{text}"));
    let (a, b) = lines[header]
        .strip_prefix("input:")
        .unwrap()
        .split_once('-')
        .unwrap();
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

#[test]
fn ordinary_lines_are_capped_only_when_diagnostics_cannot_show_them_all() {
    let dir = tempfile::tempdir().unwrap();
    let raw = format!(
        "{}error: unit zulu failed\n{}",
        distinct_lines(500),
        distinct_lines(500)
    );
    let text = compress_v3(dir.path(), raw.as_bytes(), 4096);
    assert!(text.contains(" error: unit zulu failed\n"), "{text}");
    assert!(!text.contains("omitted_units=0"), "{text}");
    assert!(text.len() < 2048, "{} bytes:\n{text}", text.len());
    assert!(text.contains("input:1-"), "{text}");

    // An ordinary listing without diagnostics still fills the budget.
    let listing = distinct_lines(1000);
    let text = compress_v3(dir.path(), listing.as_bytes(), 4096);
    assert!(text.len() > 3500, "{} bytes:\n{text}", text.len());
    assert!(!text.contains("omitted_units=0"), "{text}");
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

/// A mix of the shapes v3 treats specially: repeats, escapes, carriage-return
/// rewrites, oversized lines, diagnostics, non-ASCII and near-identical lines.
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

/// Every label in a v3 view resolves to the source line it names, and the text
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
            compress::automatic_lazy(raw.as_bytes(), Some(dir.path().into()), 4096, Version::V3)
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
            let (label, body) = match line.split_once(' ') {
                Some((label, body)) => (label, Some(body)),
                None => (line, None),
            };
            let numbers = label
                .strip_prefix("input:")
                .expect("every unit is labelled");
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
    let text = compress_v3(dir.path(), raw.as_bytes(), 4096);
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
