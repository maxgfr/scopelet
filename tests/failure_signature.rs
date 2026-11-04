//! What counts as the same failure for a loop note.
use scopelet::compress::failure_signature;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/bench/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// Real failing outputs have a signature labelled by what failed; diffs,
/// logs, searches, JSON and sources have none.
#[test]
fn real_failures_have_a_signature_and_other_outputs_none() {
    for (name, label) in [
        (
            "cargo_test.txt",
            "session::tests::refresh_rotates_expiring_token",
        ),
        ("go_test.txt", "TestRefresh"),
        ("jest.txt", "session › refresh › keeps the user id"),
        ("tsc.txt", "src/api/index.ts(10,5): error TS7006"),
        ("java_npe.txt", ""),
    ] {
        let (hash, shown) = failure_signature(&fixture(name)).unwrap_or_else(|| panic!("{name}"));
        assert_eq!(hash.len(), 16, "{name}");
        assert!(shown.starts_with(label), "{name}: {shown}");
        assert!(shown.chars().count() <= 81, "{name}: {shown}");
    }
    assert!(failure_signature(&fixture("pytest_traceback.txt")).is_some());
    for name in [
        "gh_api.txt",
        "git_diff.txt",
        "git_log_p.txt",
        "kubectl_json.txt",
        "rg_context.txt",
        "rust_large.txt",
        "rust_small.txt",
        "service_log.txt",
    ] {
        assert_eq!(failure_signature(&fixture(name)), None, "{name}");
    }
}

/// Passing runs of the usual runners report no failure, even in the
/// summaries that count failures (`0 failed`).
#[test]
fn passing_runs_have_no_signature() {
    for output in [
        "running 9 tests\ntest a ... ok\n\ntest result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s\n",
        "============================= 12 passed in 0.31s ==============================\n",
        "PASS  src/a.test.js\nTests:       0 failed, 12 passed, 12 total\n",
        "ok  \texample.com/pkg\t0.012s\n--- PASS: TestA (0.00s)\nPASS\n",
        "Found 0 errors. Watching for file changes.\nwarning: unused variable `x`\n",
        "",
    ] {
        assert_eq!(failure_signature(output), None, "{output}");
    }
}

/// Timings, counters, addresses and colors do not make a new failure; a
/// different failing test or message does.
#[test]
fn a_signature_ignores_timings_and_follows_what_failed() {
    let pytest = |seconds: &str, test: &str| {
        format!(
            "\x1b[31mFAILED\x1b[0m tests/test_a.py::{test} - assert 1 == 2\nE   assert 1 == 2\n=== 1 failed, 4 passed in {seconds}s ===\n"
        )
    };
    let first = failure_signature(&pytest("0.31", "test_x")).unwrap();
    assert_eq!(
        failure_signature(&pytest("1.07", "test_x")).unwrap().0,
        first.0
    );
    assert_ne!(
        failure_signature(&pytest("0.31", "test_y")).unwrap().0,
        first.0
    );
    let rustc = |code: &str| {
        format!(
            "error[{code}]: mismatched types\n --> src/a.rs:3:5\nerror: could not compile `a` (lib) due to 1 previous error\n"
        )
    };
    let a = failure_signature(&rustc("E0308")).unwrap();
    assert_eq!(a.1, "error[E0308]: mismatched types");
    assert_ne!(
        failure_signature(&rustc("E0308").replace("mismatched types", "unresolved import"))
            .unwrap()
            .0,
        a.0
    );
    // CRLF endings read the same.
    assert_eq!(
        failure_signature(&pytest("0.31", "test_x").replace('\n', "\r\n"))
            .unwrap()
            .0,
        first.0
    );
}
