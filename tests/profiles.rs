//! Command profiles: which commands hooks recognize, and the compact-v3
//! budget and exact-size threshold each profile gives its output.
use scopelet::commands::{Profile, classify, invokes_wrapper, profile};
use serde_json::{Value, json};

fn args(command: &str) -> Vec<String> {
    command.split_whitespace().map(str::to_owned).collect()
}

#[test]
fn safe_families_are_recognized_with_their_profile() {
    for (command, expected) in [
        ("python3 -m pytest -x", Profile::Tests),
        ("python -m unittest discover", Profile::Tests),
        ("python3 -m mypy src", Profile::Tests),
        ("uv run pytest", Profile::Tests),
        ("npx tsc --noEmit", Profile::Tests),
        ("npx vitest run", Profile::Tests),
        ("npx prettier --check .", Profile::Tests),
        ("npx playwright test", Profile::Tests),
        ("pnpm exec eslint .", Profile::Tests),
        ("bunx jest", Profile::Tests),
        ("bun test", Profile::Tests),
        ("make test", Profile::Tests),
        ("make lint", Profile::Tests),
        ("cargo nextest run", Profile::Tests),
        ("cargo doc --no-deps", Profile::Tests),
        ("cargo fmt --check", Profile::Tests),
        ("go build ./...", Profile::Tests),
        ("go vet ./...", Profile::Tests),
        ("ruff check .", Profile::Tests),
        ("ruff format --check", Profile::Tests),
        ("mypy src", Profile::Tests),
        ("./gradlew test", Profile::Tests),
        ("mvn -q verify", Profile::Tests),
        ("dotnet build", Profile::Tests),
        ("npm test -w packages/api", Profile::Tests),
        ("pytest -x tests", Profile::Tests),
        ("git log -p", Profile::Git),
        ("git -C repo log --stat", Profile::Git),
        ("git --no-pager blame src/lib.rs", Profile::Git),
        ("git grep -n token", Profile::Git),
        ("git log --follow src/lib.rs", Profile::Git),
        ("rg 'fn \\w+' src", Profile::Search),
        ("find . -name *.rs -type f", Profile::Search),
        ("ls -R src", Profile::Search),
        ("ls -laR", Profile::Search),
        ("tree src", Profile::Search),
        ("jq .items data.json", Profile::Logs),
        ("docker logs api", Profile::Logs),
        ("kubectl logs pod/api --tail 200", Profile::Logs),
        ("cat src/lib.rs", Profile::FileRead),
        ("nl src/lib.rs", Profile::FileRead),
        ("head -n 50 src/lib.rs", Profile::FileRead),
        ("tail -200 build.log", Profile::FileRead),
        ("sed -n 10,80p src/lib.rs", Profile::FileRead),
        ("cat scopelet.txt", Profile::FileRead),
        ("rg rtk src", Profile::Search),
    ] {
        assert_eq!(profile(&args(command)), Some(expected), "{command}");
    }
}

#[test]
fn writing_waiting_or_unknown_commands_are_not_recognized() {
    for command in [
        "find . -delete",
        "find . -exec rm {} ;",
        "find . -execdir ls ;",
        "find . -ok rm ;",
        "find . -fprint out",
        "sed -i s/a/b/ file",
        "sed -n 1p",
        "tail -f build.log",
        "tail --follow=name build.log",
        "head",
        "docker logs -f api",
        "kubectl logs --follow api",
        "git -p log",
        "git --paginate log",
        "git log --ext-diff",
        "cargo fmt",
        "npx some-tool",
        "uv run ./script",
        "python3 -m http.server",
        "make install",
        "ls -la",
        "jq .",
        "tee out",
        "xargs rm",
        "awk 1 file",
        "scopelet run --auto -- npm test",
        "/usr/local/bin/rtk cargo test",
        "node /x/skills/scopelet/scripts/scopelet.mjs expand last",
        "npm test -- --watch",
    ] {
        assert_eq!(profile(&args(command)), None, "{command}");
    }
}

#[test]
fn profile_budgets_grow_with_what_the_output_is_for() {
    assert_eq!(Profile::Tests.budget(), 4096);
    for profile in [Profile::Search, Profile::Git, Profile::Logs] {
        assert_eq!(profile.budget(), 8192);
        assert_eq!(profile.small(), 2048);
    }
    assert_eq!(Profile::FileRead.budget(), 16384);
    assert_eq!(Profile::FileRead.small(), 16384);
}

#[test]
fn command_lines_are_classified_and_wrappers_found_by_words() {
    assert_eq!(classify("cd sub && cargo test 2>&1"), Some(Profile::Tests));
    assert_eq!(classify("RUST_LOG=1 rg x | head"), Some(Profile::Search));
    assert_eq!(classify("echo $HOME"), None);
    assert!(invokes_wrapper("scopelet expand last"));
    assert!(invokes_wrapper("cd x && rtk cargo test"));
    assert!(invokes_wrapper(
        "node skills/scopelet/scripts/scopelet.mjs doctor"
    ));
    assert!(invokes_wrapper("FOO=$X scopelet doctor"));
    assert!(!invokes_wrapper("cat ~/notes/scopelet-ideas.md"));
    assert!(!invokes_wrapper("rg rtk src"));
}

/// A word per number, without digits, so lines do not fold by template.
fn word(i: usize) -> String {
    i.to_string()
        .bytes()
        .map(|b| (b'a' + (b - b'0')) as char)
        .collect()
}

fn hook(dir: &std::path::Path, agent: &str, event: Value) -> Value {
    let output = assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("scopelet"))
        .env("SCOPELET_COMPACT_VERSION", "3")
        .env("SCOPELET_CONFIG_DIR", dir.join("config"))
        .env("SCOPELET_CACHE_DIR", dir.join("cache"))
        .args(["hook", agent])
        .write_stdin(event.to_string())
        .output()
        .unwrap();
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn codex_rewrites_carry_the_profile_budget_in_v3() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("big.rs"), "fn x() {}\n".repeat(2000)).unwrap();
    std::fs::write(dir.path().join("mid.rs"), "fn x() {}\n".repeat(1000)).unwrap();
    let command = |line: &str| {
        let event = json!({"hook_event_name":"PreToolUse","tool_name":"Bash","cwd":dir.path(),"tool_input":{"command":line}});
        hook(dir.path(), "codex", event)["hookSpecificOutput"]["updatedInput"]["command"]
            .as_str()
            .map(str::to_owned)
    };
    assert!(
        command("cargo test")
            .unwrap()
            .contains("--profile tests -- ")
    );
    assert!(
        command("rg needle src")
            .unwrap()
            .contains("--profile search -- ")
    );
    assert!(
        command("cat big.rs")
            .unwrap()
            .contains("--profile file-read -- ")
    );
    // A 10 KB source read is below the FileRead threshold: it stays native.
    assert_eq!(command("cat mid.rs"), None);
}

#[test]
fn claude_keeps_ordinary_source_reads_whole_and_compresses_large_ones() {
    let dir = tempfile::tempdir().unwrap();
    let event = |command: &str, stdout: String| json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":command},"tool_response":{"stdout":stdout,"stderr":"","interrupted":false,"isImage":false}});
    let source: String = (0..300)
        .map(|i| format!("    let value_{i} = compute({i});\n"))
        .collect();
    assert!(source.len() > 8192 && source.len() < 16384);
    assert_eq!(
        hook(
            dir.path(),
            "claude",
            event("cat src/lib.rs", source.clone())
        ),
        json!({})
    );
    // The same bytes from a test run are compressed to the tests budget.
    let replaced = hook(dir.path(), "claude", event("cargo test", source.clone()));
    let stdout = replaced["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
        .as_str()
        .unwrap();
    assert!(stdout.len() <= 4096, "{}", stdout.len());
    // A search keeps up to its larger budget.
    let log: String = (0..2000)
        .map(|i| format!("src/{}.rs: hit {}\n", word(i), word(i * 7)))
        .collect();
    let replaced = hook(dir.path(), "claude", event("rg hit src", log));
    let stdout = replaced["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
        .as_str()
        .unwrap();
    assert!(
        stdout.len() > 4096 && stdout.len() <= 8192,
        "{}",
        stdout.len()
    );
}

#[test]
fn run_auto_accepts_a_byte_budget() {
    let dir = tempfile::tempdir().unwrap();
    let lines: String = (0..3000)
        .map(|i| format!("line {} ready\n", word(i)))
        .collect();
    let file = dir.path().join("lines.txt");
    std::fs::write(&file, lines).unwrap();
    let script = format!("cat {}", file.display());
    for (budget, limit) in [("1024", 1024), ("16384", 16384)] {
        let output = assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("scopelet"))
            .env("SCOPELET_COMPACT_VERSION", "3")
            .args(["--cache-dir"])
            .arg(dir.path())
            .args([
                "run",
                "--auto",
                "--max-bytes",
                budget,
                "--",
                "/bin/sh",
                "-c",
                &script,
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stdout.starts_with(b"[scopelet compact-v3"));
        assert!(output.stdout.len() <= limit && output.stdout.len() > limit / 2);
    }
}

/// A word like `error` in source code is not an outcome: a file read fills
/// its budget with the file instead of stopping where a log's evidence would.
#[test]
fn a_file_read_is_not_ranked_by_diagnostic_words() {
    use scopelet::compress::{self, Version};
    let dir = tempfile::tempdir().unwrap();
    let mut source: String = (0..1500)
        .map(|i| format!("    let {} = compute_{}();\n", word(i), word(i + 7)))
        .collect();
    source.insert_str(
        source.len() / 2,
        "    let message = \"errors are reported here\";\n",
    );
    let read = compress::automatic_profile(
        source.as_bytes(),
        Some(dir.path().into()),
        Some(Profile::FileRead),
        None,
        Version::V3,
    )
    .unwrap();
    assert!(
        read.len() > 14_000 && read.len() <= 16_384,
        "{}",
        read.len()
    );
    // The same bytes and budget as a command's output stop at the evidence.
    let output = compress::automatic_lazy(
        source.as_bytes(),
        Some(dir.path().into()),
        16_384,
        Version::V3,
    )
    .unwrap();
    assert!(output.len() < 8_000, "{}", output.len());
    // Profiles only shape v3.
    let v2 = compress::automatic_profile(
        source.as_bytes(),
        Some(dir.path().into()),
        Some(Profile::FileRead),
        None,
        Version::V2,
    )
    .unwrap();
    assert!(v2.len() <= 4096);
}
