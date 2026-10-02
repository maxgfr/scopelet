//! Repository scans: files are read on worker threads but assembled in path
//! order, and only files that produced a record keep their original.
use serde_json::{Value, json};
use std::path::Path;

fn cli(cache: &Path, workers: &str, args: &[&str], stdin: Option<&str>) -> std::process::Output {
    let mut command = assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("scopelet"));
    command
        .env("SCOPELET_WORKERS", workers)
        .arg("--cache-dir")
        .arg(cache)
        .args(args);
    if let Some(stdin) = stdin {
        command.write_stdin(stdin.to_owned());
    }
    command.output().unwrap()
}

fn ok(output: std::process::Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

/// `count` files; every tenth mentions the needle.
fn repo(root: &Path, count: usize) {
    for i in 0..count {
        let dir = root.join(format!("d{}", i % 7));
        std::fs::create_dir_all(&dir).unwrap();
        let body = if i % 10 == 0 {
            format!("file {i}\nneedle here {i}\n")
        } else {
            format!("file {i}\nhay only {i}\n")
        };
        std::fs::write(dir.join(format!("f{i:04}.txt")), body).unwrap();
    }
}

fn blobs(cache: &Path) -> usize {
    std::fs::read_dir(cache.join("blobs"))
        .unwrap()
        .filter(|e| e.as_ref().unwrap().file_name().len() == 64)
        .count()
}

#[test]
fn parallel_and_serial_scans_produce_the_same_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    repo(&root, 2000);
    let root = root.to_str().unwrap();
    let args = [
        "query",
        "--repo",
        root,
        "--find",
        "needle",
        "--max-bytes",
        "65536",
    ];
    let serial = cli(&dir.path().join("a"), "1", &args, None);
    let parallel = cli(&dir.path().join("b"), "8", &args, None);
    assert!(serial.status.success() && parallel.status.success());
    assert_eq!(serial.stdout, parallel.stdout);
    let view: Value = serde_json::from_slice(&parallel.stdout).unwrap();
    assert_eq!(view["total_records"], 200);
    // Without a search every file is a record and keeps its original.
    let all = ["query", "--repo", root, "--count"];
    let serial = ok(cli(&dir.path().join("c"), "1", &all, None));
    let parallel = ok(cli(&dir.path().join("d"), "8", &all, None));
    assert_eq!(serial["artifact"], parallel["artifact"]);
    assert_eq!(blobs(&dir.path().join("d")), 2000);
}

#[test]
fn only_matching_files_keep_their_original_and_recovery_reads_the_rest_locally() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    repo(&root, 50);
    let cache = dir.path().join("cache");
    let view = ok(cli(
        &cache,
        "4",
        &[
            "query",
            "--repo",
            root.to_str().unwrap(),
            "--find",
            "needle",
        ],
        None,
    ));
    assert_eq!(view["total_records"], 5);
    assert_eq!(blobs(&cache), 5);
    let artifact = view["artifact"].as_str().unwrap();
    let manifest = ok(cli(&cache, "4", &["expand", artifact, "--manifest"], None));
    assert_eq!(manifest["total_snapshots"], 50);

    // An unmatched file is searched through its unchanged local copy.
    let found = ok(cli(
        &cache,
        "4",
        &["expand", artifact, "--find", "hay only 7"],
        None,
    ));
    assert_eq!(found["total_records"], 1);
    // Once it changes, it is counted as not retained instead.
    std::fs::write(root.join("d0/f0007.txt"), "rewritten\n").unwrap();
    let found = ok(cli(
        &cache,
        "4",
        &["expand", artifact, "--find", "hay only 7"],
        None,
    ));
    assert_eq!(found["total_records"], 0);
    assert_eq!(found["scan_complete"], false);
    let searched = found["artifact"].as_str().unwrap();
    let manifest = ok(cli(&cache, "4", &["expand", searched, "--manifest"], None));
    assert_eq!(
        manifest["skipped"]["snapshot_not_retained"], 1,
        "{manifest}"
    );
}

#[test]
fn reusing_a_scan_checks_the_files_its_records_came_from() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    repo(&root, 30);
    let cache = dir.path().join("cache");
    let root_arg = root.to_str().unwrap();
    let search = ok(cli(
        &cache,
        "4",
        &["query", "--repo", root_arg, "--find", "needle"],
        None,
    ));
    let count = ok(cli(
        &cache,
        "4",
        &["query", "--repo", root_arg, "--find", "needle", "--count"],
        None,
    ));
    let reuse = |artifact: &Value| {
        let spec = json!({"version": 1, "source": {"type": "artifact", "id": artifact}});
        cli(
            &cache,
            "4",
            &["query", "--spec", "-"],
            Some(&spec.to_string()),
        )
    };

    // A file that contributed nothing changed: noted, not fatal.
    std::fs::write(root.join("d1/f0001.txt"), "now with a needle\n").unwrap();
    let view = ok(reuse(&search["artifact"]));
    assert_eq!(view["total_records"], 3);
    let manifest = ok(cli(
        &cache,
        "4",
        &["expand", view["artifact"].as_str().unwrap(), "--manifest"],
        None,
    ));
    assert_eq!(manifest["skipped"]["changed_since_scan"], 1, "{manifest}");
    // A computed count depends on every file.
    let output = reuse(&count["artifact"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("source changed"));

    // A file a record came from changed: the reuse is rejected.
    std::fs::write(root.join("d0/f0000.txt"), "gone\n").unwrap();
    let output = reuse(&search["artifact"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("source changed"));
}
