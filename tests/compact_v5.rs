//! Compact-v5 presentation contracts: compact-v4's selection written in
//! fewer bytes. A v5 view shows exactly the units the v4 view shows, with a
//! 12-character ID, labels without `input:` and tables as CSV rows; the
//! original bytes stay recoverable from the store.
use clap::ValueEnum;
use scopelet::{
    commands::Profile,
    compress::{self, Version},
    model::Dataset,
    store::Store,
};
use serde_json::{Value, json};

fn compress(
    dir: &std::path::Path,
    raw: &[u8],
    profile: Option<Profile>,
    version: Version,
) -> String {
    let output =
        compress::automatic_profile(raw, Some(dir.to_path_buf()), profile, None, version).unwrap();
    String::from_utf8(output.into_owned()).unwrap()
}

/// The 12 hex characters a v5 header shows.
fn short_id(text: &str) -> &str {
    text.strip_prefix("[scopelet v5 ")
        .and_then(|rest| rest.get(..12))
        .filter(|id| id.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap_or_else(|| panic!("no v5 header: {text}"))
}

fn original(dir: &std::path::Path, text: &str) -> Vec<u8> {
    let store = Store::open(Some(dir.to_path_buf())).unwrap();
    let artifact = store.resolve(short_id(text)).unwrap();
    let data: Dataset = store.dataset(&artifact).unwrap();
    store.get(&data.snapshots[0].blob).unwrap()
}

/// What the v5 grammar makes of a v4 text view: the same header facts with
/// a 12-character ID and no recovery syntax, every label without `input:`.
fn as_v5(v4: &str) -> String {
    let Some(rest) = v4.strip_prefix("[scopelet v4 ") else {
        return v4.to_owned();
    };
    let mut lines = rest.split_inclusive('\n');
    let header = lines.next().unwrap();
    let mut text = format!(
        "[scopelet v5 {}{}",
        &header[..12],
        header[16..].replace("; expand ID --find TEXT]", "]")
    );
    for line in lines {
        let line = line.strip_prefix("input:").unwrap_or(line);
        text.push_str(line);
    }
    text
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/bench/fixtures/{name}.txt",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// `scopelet compress` in compact-v5 with only the grammar parts `parts`
/// (`SCOPELET_V5_GRAMMAR`), which is read once per process.
fn compress_parts(dir: &std::path::Path, raw: &[u8], profile: Option<&str>, parts: &str) -> String {
    let out = assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("scopelet"))
        .env("SCOPELET_V5_GRAMMAR", parts)
        .env("SCOPELET_EVENTS", "0")
        .args(["--cache-dir", dir.to_str().unwrap()])
        .args(["--compact-version", "5", "compress"])
        .args(profile.map(|p| ["--profile", p]).into_iter().flatten())
        .write_stdin(raw.to_vec())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap()
}

/// The parts that only rewrite v4's selection.
const REWRITE: &str = "labels,header,tables";

/// Every benchmark fixture: with only the parts that rewrite, the v5 view is
/// the v4 view rewritten, unit for unit, and it recovers the same original.
/// `artifacts` and `outline` change the selection and are cut here.
#[test]
fn a_v5_view_shows_exactly_what_the_v4_view_shows() {
    let dir = tempfile::tempdir().unwrap();
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(format!(
            "{}/bench/fixtures/manifest.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap();
    let mut saved = 0;
    for (name, spec) in manifest.as_object().unwrap() {
        let raw = fixture(name);
        let profile = spec["profile"]
            .as_str()
            .map(|p| Profile::from_str(p, false).unwrap());
        let v4 = compress(dir.path(), &raw, profile, Version::V4);
        let v5 = compress_parts(dir.path(), &raw, spec["profile"].as_str(), REWRITE);
        assert_eq!(v5, as_v5(&v4), "{name}");
        if v5.starts_with("[scopelet v5 ") {
            assert_eq!(original(dir.path(), &v5), raw, "{name}");
            assert!(v5.len() < v4.len(), "{name}");
            saved += v4.len() - v5.len();
        }
    }
    assert!(saved > 2000, "{saved} bytes saved");
}

#[test]
fn the_header_has_a_short_id_and_no_recovery_syntax() {
    let dir = tempfile::tempdir().unwrap();
    let complete = format!(
        "start\n{}fatal error: rare root cause\nend\n",
        "error: retry failed\nworking\n".repeat(1000)
    );
    let text = compress(dir.path(), complete.as_bytes(), None, Version::V5);
    assert_eq!(
        text.lines().next().unwrap(),
        format!("[scopelet v5 {} complete]", short_id(&text))
    );
    assert!(
        text.contains("\n2002 fatal error: rare root cause\n"),
        "{text}"
    );
    assert_eq!(original(dir.path(), &text), complete.as_bytes());

    let partial: String = (0..3000)
        .map(|i| {
            let word: String = i
                .to_string()
                .bytes()
                .map(|b| (b'a' + (b - b'0')) as char)
                .collect();
            format!("{word} unit ready\n")
        })
        .collect();
    let text = compress(dir.path(), partial.as_bytes(), None, Version::V5);
    let header = text.lines().next().unwrap();
    assert!(
        header.starts_with(&format!("[scopelet v5 {} omitted=", short_id(&text)))
            && header.ends_with("; gaps are not absence]"),
        "{header}"
    );
    assert!(text.lines().nth(1).unwrap().starts_with("1-"), "{text}");
    // `expand` resolves the 12 characters like any unique prefix.
    let found = assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("scopelet"))
        .args(["--cache-dir", dir.path().to_str().unwrap(), "expand"])
        .args([short_id(&text), "--find", "unit ready"])
        .output()
        .unwrap();
    assert!(found.status.success(), "{found:?}");
}

/// A search keeps its per-file blocks, labelled by line alone.
#[test]
fn search_blocks_are_labelled_by_line() {
    let dir = tempfile::tempdir().unwrap();
    let raw: String = (0..600)
        .map(|i| {
            let word: String = i
                .to_string()
                .bytes()
                .map(|b| (b'a' + (b - b'0')) as char)
                .collect();
            format!(
                "src/mod{}.rs:{}:    let {word} = refresh();\n",
                i / 20,
                2 + i % 20
            )
        })
        .collect();
    let v4 = compress(
        dir.path(),
        raw.as_bytes(),
        Some(Profile::Search),
        Version::V4,
    );
    let v5 = compress(
        dir.path(),
        raw.as_bytes(),
        Some(Profile::Search),
        Version::V5,
    );
    assert!(v4.contains("\ninput:1-"), "{v4}");
    assert!(v5.contains(" file=src/mod0.rs"), "{v5}");
    assert!(!v5.contains("input:"), "{v5}");
    assert_eq!(v5, as_v5(&v4));
}

/// Cargo totals keep what they sum on the line itself: without it, live
/// sessions trusted their own partial sum of the results shown, and the 14
/// tokens it costs are not worth that.
#[test]
fn cargo_totals_keep_their_explanation() {
    let dir = tempfile::tempdir().unwrap();
    let mut raw = String::new();
    for binary in 0..6 {
        raw.push_str(&format!(
            "     Running unittests src/b{binary}.rs\n{}test result: ok. 10 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out\n\n",
            (0..40)
                .map(|i| format!("test m{binary}::t{i} ... ok\n"))
                .collect::<String>()
        ));
    }
    let v4 = compress(
        dir.path(),
        raw.as_bytes(),
        Some(Profile::Tests),
        Version::V4,
    );
    let v5 = compress(
        dir.path(),
        raw.as_bytes(),
        Some(Profile::Tests),
        Version::V5,
    );
    assert!(
        v4.contains("totals runs=6 passed=60 failed=0 ignored=6 (every test result"),
        "{v4}"
    );
    assert_eq!(
        v5.lines().nth(1),
        Some(
            "totals runs=6 passed=60 failed=0 ignored=6 (every test result line, omitted ones included)"
        )
    );
    assert_eq!(v5, as_v5(&v4));
}

/// One cell of a v5 CSV row read back as JSON: a quoted cell is a JSON
/// string, a bare one a JSON scalar when it parses as one, else a string.
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
fn cells(line: &str) -> Vec<Value> {
    let (mut cells, mut start, mut quoted, mut escaped) = (Vec::new(), 0, false, false);
    for (i, c) in line.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ',' if !quoted => {
                cells.push(cell(&line[start..i]));
                start = i + 1;
            }
            _ => {}
        }
    }
    cells.push(cell(&line[start..]));
    cells
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
    let v4 = compress(dir.path(), raw.as_bytes(), None, Version::V4);
    let v5 = compress(dir.path(), raw.as_bytes(), None, Version::V5);
    assert!(v5.len() < v4.len(), "{} >= {}", v5.len(), v4.len());
    let mut lines = v5.lines();
    let header = lines.next().unwrap();
    assert!(header.contains(" omitted="), "{header}");
    let table = lines.next().unwrap();
    let (columns, counts) = table
        .strip_prefix("table ")
        .and_then(|t| t.split_once(" rows="))
        .unwrap_or_else(|| panic!("{v5}"));
    let columns: Vec<String> = cells(columns)
        .into_iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    let (shown, indices) = counts.split_once(" indices=").unwrap();
    let (shown, total) = shown.split_once('/').unwrap();
    assert_eq!(total, "400");
    let indices: Vec<usize> = indices
        .split(',')
        .flat_map(|part| match part.split_once('-') {
            Some((a, b)) => (a.parse().unwrap()..=b.parse().unwrap()).collect::<Vec<usize>>(),
            None => vec![part.parse().unwrap()],
        })
        .collect();
    assert_eq!(indices.len(), shown.parse::<usize>().unwrap());
    let constant = lines.next().unwrap();
    assert_eq!(
        constant,
        "constant exact=900719925474099312345,region=eu-west"
    );
    let body: Vec<&str> = lines.collect();
    assert_eq!(body.len(), indices.len(), "{v5}");
    for (line, &i) in body.iter().zip(&indices) {
        let values = cells(line);
        assert_eq!(values.len(), columns.len(), "{line}");
        for (column, value) in columns.iter().zip(&values) {
            assert_eq!(&rows[i][column], value, "row {i} {column}: {line}");
        }
    }
    // The v4 view shows the same records.
    let v4_indices: Value =
        serde_json::from_str::<Value>(v4.lines().nth(1).unwrap().strip_prefix("table ").unwrap())
            .unwrap()["indices"]
            .clone();
    assert_eq!(v4_indices, json!(indices));
    assert_eq!(original(dir.path(), &v5), raw.as_bytes());
}

/// A table with a nested cell stays a JSON table: CSV would split it.
#[test]
fn nested_cells_keep_the_json_table() {
    let dir = tempfile::tempdir().unwrap();
    let raw = (0..300)
        .map(|i| json!({"id": i, "tags": ["a", "b"], "note": format!("n{i}")}).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let v5 = compress(dir.path(), raw.as_bytes(), None, Version::V5);
    assert!(v5.lines().nth(1).unwrap().starts_with("table {"), "{v5}");
}

/// A repeat reference carries the short ID and the facts, nothing more.
#[test]
fn a_repeat_reference_is_short() {
    let dir = tempfile::tempdir().unwrap();
    let log: String = (0..400)
        .map(|i| format!("worker {i} finished batch\n"))
        .collect();
    let event = json!({"hook_event_name":"PostToolUse","session_id":"v5r","tool_name":"Bash","tool_input":{"command":"npm test"},"tool_response":{"stdout":log,"stderr":"","interrupted":false,"isImage":false}});
    let run = || -> Value {
        let out = assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("scopelet"))
            .env("SCOPELET_COMPACT_VERSION", "5")
            .env("SCOPELET_CONFIG_DIR", dir.path().join("config"))
            .env("SCOPELET_CACHE_DIR", dir.path().join("cache"))
            .args(["hook", "claude"])
            .write_stdin(event.to_string())
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout).unwrap()
    };
    let first = run()["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
        .as_str()
        .unwrap()
        .to_owned();
    let second = run()["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        second,
        format!(
            "[scopelet v5 {} repeat: identical to output already shown in this conversation (400 lines)]\n",
            short_id(&first)
        )
    );
}

/// Each part of the grammar can be measured alone: with only `labels`, the
/// header keeps v4's ID and recovery syntax.
#[test]
fn grammar_parts_can_be_measured_alone() {
    let dir = tempfile::tempdir().unwrap();
    let raw = fixture("go_test");
    let run = |parts: &str| -> String {
        let out = assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("scopelet"))
            .env("SCOPELET_V5_GRAMMAR", parts)
            .env("SCOPELET_EVENTS", "0")
            .args(["--cache-dir", dir.path().to_str().unwrap()])
            .args(["--compact-version", "5", "compress", "--profile", "tests"])
            .write_stdin(raw.clone())
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    };
    let labels = run("labels");
    let header = labels.lines().next().unwrap();
    assert!(header.starts_with("[scopelet v5 "), "{header}");
    assert!(header.ends_with("; expand ID --find TEXT]"), "{header}");
    assert_eq!(header.split(' ').nth(2).unwrap().len(), 16, "{header}");
    assert!(!labels.contains("\ninput:"), "{labels}");
    let headers = run("header");
    assert!(headers.contains("\ninput:"), "{headers}");
    assert!(headers.lines().next().unwrap().ends_with("not absence]"));
    assert_eq!(run(""), run("none"));
    assert!(run("").contains("\ninput:"));
}

fn word(i: usize) -> String {
    i.to_string()
        .bytes()
        .map(|b| (b'a' + (b - b'0')) as char)
        .collect()
}

/// A listing folds each artifact directory to one counted line where its
/// first entry was, even when the listing fits; the entries count as
/// omitted and the original stays recoverable.
#[test]
fn artifact_directories_fold_to_one_counted_line() {
    let dir = tempfile::tempdir().unwrap();
    let mut find = String::from(".\n./src\n./node_modules\n");
    for i in 0..60 {
        find.push_str(&format!("./src/mod_{}.py\n", word(i)));
        if i % 3 == 0 {
            find.push_str(&format!("./node_modules/pkg-{}/index.js\n", word(i)));
            find.push_str(&format!("./node_modules/pkg-{}/package.json\n", word(i)));
        }
    }
    for i in 0..5 {
        find.push_str(&format!(
            "./src/__pycache__/mod_{}.cpython-312.pyc\n",
            word(i)
        ));
    }
    let v5 = compress(
        dir.path(),
        find.as_bytes(),
        Some(Profile::Search),
        Version::V5,
    );
    assert!(
        v5.contains("\n./node_modules/ files=40 artifacts omitted\n4 ./src/mod_a.py\n"),
        "{v5}"
    );
    assert!(
        v5.contains("\n./src/__pycache__/ files=5 artifacts omitted\n"),
        "{v5}"
    );
    assert!(!v5.contains("pkg-") && !v5.contains(".pyc"), "{v5}");
    // 40 + 5 entries and the line naming node_modules itself.
    assert!(v5.lines().next().unwrap().contains(" omitted=46;"), "{v5}");
    for line in find.lines().filter(|l| l.starts_with("./src/mod_")) {
        assert!(v5.contains(&format!(" {line}\n")), "{line}: {v5}");
    }
    assert_eq!(original(dir.path(), &v5), find.as_bytes());
    // The parts that only rewrite leave the listing as v4 shows it.
    let v4 = compress(
        dir.path(),
        find.as_bytes(),
        Some(Profile::Search),
        Version::V4,
    );
    assert_eq!(
        compress_parts(dir.path(), find.as_bytes(), Some("search"), REWRITE),
        as_v5(&v4)
    );

    // `tree` and `ls -R` name the same directories.
    let mut tree = String::from(".\n");
    tree.push_str("├── target\n│   └── debug\n");
    for i in 0..40 {
        tree.push_str(&format!("│       ├── dep_{}.rlib\n", word(i)));
    }
    tree.push_str("└── src\n");
    for i in 0..40 {
        tree.push_str(&format!("    ├── module_{}.rs\n", word(i)));
    }
    let v5 = compress(
        dir.path(),
        tree.as_bytes(),
        Some(Profile::Search),
        Version::V5,
    );
    assert!(
        v5.contains("\ntarget/ files=41 artifacts omitted\n"),
        "{v5}"
    );
    assert!(!v5.contains("dep_") && v5.contains("module_ba.rs"), "{v5}");

    let mut ls = String::from(".:\nREADME.md\nbuild\nsrc\n\n./build:\n");
    for i in 0..150 {
        ls.push_str(&format!("bundle_{}.js\n", word(i)));
    }
    ls.push_str("\n./src:\n");
    for i in 0..150 {
        ls.push_str(&format!("page_{}.ts\n", word(i)));
    }
    let v5 = compress(
        dir.path(),
        ls.as_bytes(),
        Some(Profile::Search),
        Version::V5,
    );
    assert!(
        v5.contains("\n./build/ files=150 artifacts omitted\n"),
        "{v5}"
    );
    assert!(!v5.contains("bundle_") && v5.contains("README.md"), "{v5}");
}

/// A listing without an artifact directory is shown as v4 shows it.
#[test]
fn a_listing_without_artifacts_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let listing: String = (0..300)
        .map(|i| format!("./src/area_{}/file_{}.rs\n", word(i / 30), word(i)))
        .collect();
    for raw in [listing.clone(), listing[..3000].to_owned()] {
        let v4 = compress(
            dir.path(),
            raw.as_bytes(),
            Some(Profile::Search),
            Version::V4,
        );
        let v5 = compress(
            dir.path(),
            raw.as_bytes(),
            Some(Profile::Search),
            Version::V5,
        );
        assert_eq!(v5, as_v5(&v4));
    }
}

/// A large file read keeps every definition line, and the lines naming a
/// term of the task, where v4 fills its budget from the head and tail.
#[test]
fn a_large_file_read_keeps_its_outline() {
    let dir = tempfile::tempdir().unwrap();
    let mut source =
        String::from("\"\"\"Request handlers.\"\"\"\n\n\nclass Router:\n    pass\n\n\n");
    for i in 0..80 {
        if i % 10 == 0 {
            source.push_str("@register\n");
        }
        source.push_str(&format!("def handler_{}(request, context):\n", word(i)));
        for k in 0..9 {
            source.push_str(&if i == 31 {
                format!(
                    "    {} = settle_ledger(request.{})\n",
                    word(k),
                    word(k + 50)
                )
            } else {
                format!(
                    "    value_{k} = request.get(\"{}\") or context.default_{k}\n",
                    word(i * 7 + k)
                )
            });
        }
        source.push_str("    return value_0\n\n\n");
    }
    assert!(source.len() > 40 * 1024, "{}", source.len());
    let signatures: Vec<&str> = source
        .lines()
        .filter(|l| l.starts_with("def ") || l.starts_with("class ") || l.starts_with('@'))
        .collect();
    let anchors = vec!["settle_ledger".to_owned()];
    let view = |version| {
        let output = compress::automatic_anchored(
            "cli",
            source.as_bytes(),
            Some(dir.path().to_path_buf()),
            Some(Profile::FileRead),
            None,
            version,
            &anchors,
        );
        String::from_utf8(output.into_owned()).unwrap()
    };
    let v5 = view(Version::V5);
    for signature in &signatures {
        assert!(v5.contains(signature), "{signature}: {v5}");
    }
    // The anchored function's body, every line of it.
    let body: Vec<&str> = source
        .lines()
        .filter(|l| l.contains("settle_ledger"))
        .collect();
    assert_eq!(body.len(), 9);
    for line in body {
        assert!(v5.contains(line), "{line}: {v5}");
    }
    assert_eq!(original(dir.path(), &v5), source.as_bytes());
    // The rule is what shows them: v4 leaves signatures out.
    let v4 = view(Version::V4);
    assert!(signatures.iter().any(|s| !v4.contains(s)), "{v4}");
    // The budget is spent as in v4, not capped as if beside a failure.
    assert!(v5.len() * 10 >= v4.len() * 8, "{} {}", v5.len(), v4.len());
}
