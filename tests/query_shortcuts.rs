//! Query shortcuts: group order, --top, --unique, --include/--exclude,
//! --regex/--ignore-case, and flags that would be silently ignored.
use serde_json::{Value, json};

fn cli(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("scopelet"))
        .env_remove("SCOPELET_COMPACT_VERSION")
        .arg("--cache-dir")
        .arg(dir.join("cache"))
        .args(args)
        .output()
        .unwrap()
}

fn ok(output: std::process::Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn rejected(output: std::process::Output, hint: &str) {
    assert!(!output.status.success(), "accepted: {output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(hint), "{hint:?} not in {stderr}");
}

/// Events whose suites occur 1, 3 and 2 times, in that key order.
fn events(dir: &std::path::Path) -> String {
    let path = dir.join("events.jsonl");
    let rows: Vec<String> = ["a", "b", "b", "c", "b", "c"]
        .iter()
        .map(|suite| json!({"suite": suite, "status": "failed"}).to_string())
        .collect();
    std::fs::write(&path, rows.join("\n")).unwrap();
    path.to_string_lossy().into_owned()
}

fn keys(view: &Value) -> Vec<(String, u64)> {
    view["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["value"]["key"].as_str().unwrap().to_owned(),
                r["value"]["count"].as_u64().unwrap(),
            )
        })
        .collect()
}

#[test]
fn the_group_shortcut_lists_the_largest_groups_first() {
    let dir = tempfile::tempdir().unwrap();
    let file = events(dir.path());
    let pairs = |v: &[(&str, u64)]| -> Vec<(String, u64)> {
        v.iter().map(|(k, n)| (k.to_string(), *n)).collect()
    };
    // Streamed JSONL aggregation and the materialized pipeline agree.
    for format in [&["--format", "jsonl"][..], &["--format", "json"][..]] {
        let path = if format[1] == "json" {
            let json = dir.path().join("events.json");
            let rows: Vec<Value> = std::fs::read_to_string(&file)
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect();
            std::fs::write(&json, Value::Array(rows).to_string()).unwrap();
            json.to_string_lossy().into_owned()
        } else {
            file.clone()
        };
        let mut args = vec!["query", "--file", &path];
        args.extend_from_slice(format);
        args.extend_from_slice(&["--group", "/suite"]);
        let view = ok(cli(dir.path(), &args));
        assert_eq!(keys(&view), pairs(&[("b", 3), ("c", 2), ("a", 1)]));
    }
    // A spec keeps key order unless it asks for counts.
    for (order, expected) in [
        (None, pairs(&[("a", 1), ("b", 3), ("c", 2)])),
        (Some("count"), pairs(&[("b", 3), ("c", 2), ("a", 1)])),
        (Some("key"), pairs(&[("a", 1), ("b", 3), ("c", 2)])),
    ] {
        let mut group = json!({"op": "group", "pointer": "/suite"});
        if let Some(order) = order {
            group["order"] = json!(order);
        }
        let spec = dir.path().join("spec.json");
        let request = json!({"version": 1, "source": {"type": "file", "path": file, "format": "jsonl"}, "operations": [group]});
        std::fs::write(&spec, request.to_string()).unwrap();
        let view = ok(cli(
            dir.path(),
            &["query", "--spec", spec.to_str().unwrap()],
        ));
        assert_eq!(keys(&view), expected, "{order:?}");
    }
}

#[test]
fn top_limits_the_view_while_the_artifact_keeps_every_record() {
    let dir = tempfile::tempdir().unwrap();
    let file = events(dir.path());
    let view = ok(cli(
        dir.path(),
        &[
            "query", "--file", &file, "--format", "jsonl", "--group", "/suite", "--top", "1",
        ],
    ));
    assert_eq!(keys(&view), vec![("b".to_owned(), 3)]);
    assert_eq!(view["total_records"], 3);
    assert_eq!(view["omitted_records"], 2);
    assert_eq!(view["next_offset"], 1);
    assert_eq!(view["display_complete"], false);
    let artifact = view["artifact"].as_str().unwrap();
    let all = ok(cli(dir.path(), &["expand", artifact]));
    assert_eq!(all["total_records"], 3);
    // A limit at or above the result changes nothing.
    let whole = ok(cli(
        dir.path(),
        &[
            "query", "--file", &file, "--format", "jsonl", "--group", "/suite", "--top", "9",
        ],
    ));
    assert_eq!(whole["display_complete"], true);
    assert_eq!(whole["artifact"], artifact);
}

#[test]
fn project_then_group_uses_the_projected_key() {
    let dir = tempfile::tempdir().unwrap();
    let file = events(dir.path());
    let view = ok(cli(
        dir.path(),
        &[
            "query",
            "--file",
            &file,
            "--format",
            "jsonl",
            "--project",
            "/suite",
            "--group",
            "/suite",
        ],
    ));
    assert_eq!(keys(&view)[0], ("b".to_owned(), 3));
}

#[test]
fn unique_include_exclude_regex_and_ignore_case_shortcuts() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/a.rs"), "fn main() { Panic!(\"x\") }\n").unwrap();
    std::fs::write(repo.join("src/b.txt"), "panicked here\n").unwrap();
    std::fs::write(repo.join("notes.md"), "PANIC in notes\n").unwrap();
    let repo = repo.to_str().unwrap();
    let count = |args: &[&str]| -> u64 {
        let mut all = vec!["query", "--repo", repo];
        all.extend_from_slice(args);
        all.push("--count");
        ok(cli(dir.path(), &all))["records"][0]["value"]["count"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(count(&["--find", "panic"]), 1);
    assert_eq!(count(&["--find", "panic", "--ignore-case"]), 3);
    assert_eq!(count(&["--find", "^panic", "--regex", "--ignore-case"]), 2);
    assert_eq!(
        count(&["--find", "panic", "--ignore-case", "--include", "src/**"]),
        2
    );
    assert_eq!(
        count(&["--find", "panic", "--ignore-case", "--exclude", "*.md"]),
        2
    );

    let file = events(dir.path());
    let view = ok(cli(
        dir.path(),
        &[
            "query",
            "--file",
            &file,
            "--format",
            "jsonl",
            "--project",
            "/suite",
            "--unique",
        ],
    ));
    assert_eq!(view["total_records"], 3);
}

#[test]
fn flags_that_would_be_ignored_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let file = events(dir.path());
    rejected(
        cli(
            dir.path(),
            &["query", "--repo", ".", "--format", "json", "--find", "x"],
        ),
        "--file",
    );
    rejected(
        cli(dir.path(), &["query", "--repo", ".", "--context", "2"]),
        "--find",
    );
    rejected(
        cli(dir.path(), &["query", "--repo", ".", "--regex"]),
        "--find",
    );
    rejected(
        cli(
            dir.path(),
            &[
                "query", "--file", &file, "--output", "compact", "--mode", "ultra",
            ],
        ),
        "--mode applies to JSON output",
    );
    rejected(
        cli(
            dir.path(),
            &[
                "query", "--file", &file, "--output", "compact", "--top", "2",
            ],
        ),
        "--top applies to JSON output",
    );
    rejected(
        cli(dir.path(), &["query", "--file", &file, "--include", "*.rs"]),
        "cannot be used with",
    );
    rejected(
        cli(dir.path(), &["query", "--file", &file, "--top", "0"]),
        "--top must be at least 1",
    );
}
