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
    assert!(text.contains("input:1 running 1000 tests\n"), "{text}");
    assert!(text.contains("input:2 error: case 0 failed\n"), "{text}");
    assert!(
        text.contains("input:1002 final status: 1000 failed; exit=7\n"),
        "{text}"
    );
    assert!(
        text.contains("display_complete=false omitted_units="),
        "{text}"
    );
}
