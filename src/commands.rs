//! The deliberately small noninteractive command grammar shared by integrations.
use std::path::Path;

/// What a recognized command produces, which sets how much of it a
/// compact-v3 view keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Profile {
    /// Builds, tests, linters and type checkers: the outcome and its evidence.
    Tests,
    /// Searches and listings.
    Search,
    Git,
    /// Reading a file or a slice of one.
    FileRead,
    /// Service and container logs.
    Logs,
    /// Package installs: errors, warnings and the closing summary.
    Install,
}

impl Profile {
    /// The name `run --auto --profile` takes.
    pub fn name(self) -> &'static str {
        match self {
            Self::Tests => "tests",
            Self::Search => "search",
            Self::Git => "git",
            Self::FileRead => "file-read",
            Self::Logs => "logs",
            Self::Install => "install",
        }
    }
    /// Compact-v3 byte target for this output.
    pub fn budget(self) -> usize {
        match self {
            Self::Tests => 4 * 1024,
            Self::Search | Self::Git | Self::Logs => 8 * 1024,
            Self::FileRead => 16 * 1024,
            Self::Install => 2 * 1024,
        }
    }
    /// Outputs up to this size stay byte-exact: a source file of ordinary
    /// size is read whole.
    pub fn small(self) -> usize {
        match self {
            Self::FileRead => 16 * 1024,
            _ => crate::compress::SMALL,
        }
    }
}

fn name(arg: &str) -> &str {
    Path::new(arg)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(arg)
}

/// Scopelet itself, or another wrapper whose output is already shaped.
pub fn is_wrapper(args: &[String]) -> bool {
    args.first().is_some_and(|a| wrapper_program(a))
        || args.get(1).is_some_and(|a| wrapper_launcher(a))
}

/// The `scopelet` or `rtk` program.
fn wrapper_program(word: &str) -> bool {
    matches!(name(word), "scopelet" | "rtk")
}

/// The skill's Node launcher, run as `node .../scopelet.mjs`.
fn wrapper_launcher(word: &str) -> bool {
    word.ends_with("scopelet.mjs")
}

/// The profile of the first recognized command in a command line, for hosts
/// that compress after the fact; None when it does not parse or names none.
pub fn classify(command: &str) -> Option<Profile> {
    classify_for(command, true)
}

/// `classify` as a compact version sees it: before compact-v4 (`v4` false),
/// the commands first recognized with it are not recognized.
pub fn classify_for(command: &str, v4: bool) -> Option<Profile> {
    crate::shell::parse(command)?
        .commands()
        .find_map(|simple| profile_for(&simple.argv, v4))
}

/// `classify_for` for hosts that compress after the fact (Claude Code,
/// OpenCode): a line outside the strict grammar (`;`, newlines, `$`, globs)
/// is split on its operators, quotes respected, only to choose a profile,
/// never to run it. `cat`/`nl` of several files or with numbering options
/// read files here, so a code read in pieces keeps the file-read threshold.
pub fn classify_lenient(command: &str, v4: bool) -> Option<Profile> {
    if let Some(profile) = classify_for(command, v4) {
        return Some(profile);
    }
    lenient_commands(command)
        .iter()
        .find_map(|argv| profile_for(argv, v4).or_else(|| reads_files(argv)))
}

/// The program a command line runs, for the local journal: the first
/// recognized command's, else the first command's. A bare name of at most
/// 32 safe characters, never a path or an argument; `other` otherwise.
pub fn program(command: &str) -> Option<String> {
    let commands = lenient_commands(command);
    let argv = commands
        .iter()
        .find(|argv| profile(argv).is_some() || reads_files(argv).is_some())
        .or_else(|| commands.iter().find(|argv| name(&argv[0]) != "cd"))?;
    safe_program(argv)
}

/// `program` of an argument vector; `sh -c SCRIPT` is read as its script.
pub fn argv_program(args: &[String]) -> Option<String> {
    match args {
        [shell, flag, script] if matches!(name(shell), "sh" | "bash") && flag == "-c" => {
            program(script)
        }
        [] => None,
        _ => safe_program(args),
    }
}

fn safe_program(argv: &[String]) -> Option<String> {
    let program = name(argv.first()?);
    let safe = (1..=32).contains(&program.len())
        && program
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b));
    Some(if safe { program } else { "other" }.to_owned())
}

/// `cat`/`nl` of one or more files, with numbering options only.
fn reads_files(args: &[String]) -> Option<Profile> {
    let mut files = 0;
    let mut rest = args[1..].iter();
    match name(&args[0]) {
        "cat" => {
            for arg in rest {
                match arg.strip_prefix('-') {
                    Some(flags)
                        if !flags.is_empty() && flags.chars().all(|c| "nbsv".contains(c)) => {}
                    Some(_) => return None,
                    None => files += 1,
                }
            }
        }
        "nl" => {
            // Numbering options, each with its value (`-b a` or `-ba`).
            const VALUED: [&str; 6] = ["-b", "-w", "-n", "-s", "-v", "-i"];
            while let Some(arg) = rest.next() {
                if VALUED.contains(&arg.as_str()) {
                    rest.next()?;
                } else if !arg.starts_with('-') {
                    files += 1;
                } else if !arg.get(..2).is_some_and(|o| VALUED.contains(&o)) {
                    return None;
                }
            }
        }
        _ => return None,
    }
    (files > 0).then_some(Profile::FileRead)
}

/// The simple commands of a command line, split on `;`, `&`, `|` and
/// newlines outside quotes, without redirections or leading assignments.
/// Words keep `$`, globs and the like as literal text: this reading only
/// names what runs.
fn lenient_commands(command: &str) -> Vec<Vec<String>> {
    let mut commands: Vec<Vec<String>> = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut chars = command.chars().peekable();
    let finish = |words: &mut Vec<String>, word: &mut String, started: &mut bool| {
        if *started {
            words.push(std::mem::take(word));
            *started = false;
        }
    };
    let end = |commands: &mut Vec<Vec<String>>, words: &mut Vec<String>| {
        let mut argv: Vec<String> = Vec::new();
        let mut skip = false;
        for word in words.drain(..) {
            if std::mem::take(&mut skip) {
                continue;
            }
            // `>`, `2>`, `>>`, `<` alone take the next word as their target;
            // `2>&1`, `>/dev/null` carry it.
            let redirect = word.trim_start_matches(|c: char| c.is_ascii_digit() || c == '&');
            if redirect.starts_with(['<', '>']) {
                skip = redirect.trim_start_matches(['<', '>', '&']).is_empty();
                continue;
            }
            let assignment = argv.is_empty()
                && word.split_once('=').is_some_and(|(n, _)| {
                    !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                });
            if !assignment {
                argv.push(word);
            }
        }
        if !argv.is_empty() {
            commands.push(argv);
        }
    };
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => finish(&mut words, &mut word, &mut started),
            '\'' | '"' => {
                started = true;
                for inner in chars.by_ref() {
                    if inner == c {
                        break;
                    }
                    word.push(inner);
                }
            }
            '\\' => {
                started = true;
                match chars.next() {
                    Some('\n') | None => {}
                    Some(next) => word.push(next),
                }
            }
            ';' | '&' | '|' | '\n' | '\r' | '(' | ')' => {
                // `2>&1` and `&>` are redirections, not separators.
                if c == '&' && (word.ends_with('>') || chars.peek() == Some(&'>')) {
                    word.push(c);
                    started = true;
                    continue;
                }
                finish(&mut words, &mut word, &mut started);
                end(&mut commands, &mut words);
            }
            c => {
                word.push(c);
                started = true;
            }
        }
    }
    finish(&mut words, &mut word, &mut started);
    end(&mut commands, &mut words);
    commands
}

/// `profile` as a compact version sees it (see `classify_for`).
pub fn profile_for(args: &[String], v4: bool) -> Option<Profile> {
    profile(args).filter(|_| v4 || !recognized_since_v4(args))
}

/// Commands first recognized with compact-v4: package installs, `git
/// ls-files`, and `uv run` with uv options or a `python -m` tool. Earlier
/// versions leave them native, as they always did.
fn recognized_since_v4(args: &[String]) -> bool {
    if profile(args) == Some(Profile::Install) {
        return true;
    }
    let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    match args.first().map(|a| name(a)) {
        Some("git") => rest.contains(&"ls-files"),
        Some("uv") => {
            rest.first() == Some(&"run")
                && rest
                    .get(1)
                    .is_some_and(|a| a.starts_with('-') || matches!(*a, "python" | "python3"))
        }
        _ => false,
    }
}

/// Whether a command line runs Scopelet or another output wrapper, whose
/// output is already shaped. A line outside the parsed subset is checked
/// word by word.
pub fn invokes_wrapper(command: &str) -> bool {
    match crate::shell::parse(command) {
        Some(script) => script.commands().any(|simple| is_wrapper(&simple.argv)),
        None => command
            .split_whitespace()
            .any(|word| wrapper_program(word) || wrapper_launcher(word)),
    }
}

/// Tools run through `npx`, `bunx`, `pnpm exec` or `uv run` that are known
/// to finish on their own and report an outcome.
fn known_tool(args: &[String]) -> bool {
    let second = args.get(1).map(String::as_str).unwrap_or("");
    match args.first().map(|a| name(a)) {
        Some("jest" | "tsc" | "eslint" | "pytest" | "mypy" | "ruff") => true,
        // `uv run python -m pytest`: the module, as for a bare `python`.
        Some("python" | "python3") => {
            second == "-m"
                && matches!(
                    args.get(2).map(String::as_str),
                    Some("pytest" | "unittest" | "mypy")
                )
        }
        Some("vitest") => second == "run",
        Some("prettier") => args.iter().any(|a| a == "--check"),
        Some("playwright") => second == "test",
        _ => false,
    }
}

/// The command `uv run` runs: its arguments after uv's own options
/// (`--no-project`, `--with PKG`, `--python 3.12`, ...).
fn uv_run_tool(args: &[String]) -> &[String] {
    const VALUED: &[&str] = &[
        "--with",
        "--with-editable",
        "--with-requirements",
        "--python",
        "-p",
        "--project",
        "--directory",
        "--extra",
        "--group",
        "--package",
        "--env-file",
        "--index",
        "--default-index",
    ];
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        if arg == "--" {
            return &args[i + 1..];
        }
        if !arg.starts_with('-') {
            break;
        }
        i += if VALUED.contains(&arg.as_str()) { 2 } else { 1 };
    }
    &args[i.min(args.len())..]
}

/// `-f`/`-F` (alone or in a cluster such as `-fn5`) or `--follow`: the
/// command never ends on its own.
fn follows(args: &[String]) -> bool {
    has_flag(args, &['f', 'F'], &["follow", "retry"])
}

pub fn profile(args: &[String]) -> Option<Profile> {
    let name = name(args.first()?);
    let second = args.get(1).map(String::as_str).unwrap_or("");
    let has = |flag: &str| args.iter().any(|a| a == flag);
    if is_wrapper(args)
        || args.iter().any(|a| {
            matches!(a.as_str(), "--watch" | "--pdb" | "--interactive") || a.starts_with("--watch=")
        })
    {
        return None;
    }
    let tests = Some(Profile::Tests);
    let install = Some(Profile::Install);
    match name {
        "pip" | "pip3" if second == "install" => install,
        "bundle" if second == "install" => install,
        "brew" if matches!(second, "install" | "upgrade") => install,
        "uv" if second == "sync"
            || (second == "pip" && args.get(2).is_some_and(|a| a == "install")) =>
        {
            install
        }
        "cargo" => match second {
            "test" | "check" | "clippy" | "build" | "nextest" | "doc" => tests,
            "fmt" if has("--check") => tests,
            _ => None,
        },
        "python" | "python3" => {
            if has("-i") {
                None
            } else if second == "-m" && args.get(2).is_some_and(|a| a == "pip") {
                args.get(3)
                    .is_some_and(|a| a == "install")
                    .then_some(Profile::Install)
            } else if second == "-m" {
                matches!(
                    args.get(2).map(String::as_str),
                    Some("pytest" | "unittest" | "mypy")
                )
                .then_some(Profile::Tests)
            } else {
                second.ends_with(".py").then_some(Profile::Tests)
            }
        }
        "pytest" | "mypy" => tests,
        "ruff" if matches!(second, "check" | "format") && (second == "check" || has("--check")) => {
            tests
        }
        "npm" | "pnpm" | "yarn" if !has("-i") => {
            if name == "pnpm" && second == "exec" {
                return known_tool(&args[2..]).then_some(Profile::Tests);
            }
            // `yarn <bin>` runs a package binary, like `npx`.
            if name == "yarn" && known_tool(&args[1..]) {
                return tests;
            }
            if matches!(second, "install" | "i" | "ci" | "add") {
                return install;
            }
            let script = if second == "run" {
                args.get(2).map(String::as_str).unwrap_or("")
            } else {
                second
            };
            matches!(script, "test" | "build" | "lint" | "typecheck").then_some(Profile::Tests)
        }
        // `-y`/`--yes` only skips npx's install prompt.
        "npx" if matches!(second, "-y" | "--yes") => {
            known_tool(&args[2..]).then_some(Profile::Tests)
        }
        "npx" | "bunx" => known_tool(&args[1..]).then_some(Profile::Tests),
        "uv" if second == "run" => known_tool(uv_run_tool(&args[2..])).then_some(Profile::Tests),
        "bun" if second == "test" => tests,
        "bun" if matches!(second, "install" | "i" | "add") => install,
        "make" if matches!(second, "test" | "check" | "build" | "lint") => tests,
        "go" if matches!(second, "test" | "build" | "vet") => tests,
        "gradle" | "gradlew" | "mvn" | "mvnw" | "dotnet"
            if args[1..]
                .iter()
                .any(|a| matches!(a.as_str(), "test" | "build" | "check" | "verify")) =>
        {
            tests
        }
        "node"
            if second == "--test"
                && !args
                    .iter()
                    .any(|a| a.starts_with("--inspect") || a == "-i" || a == "--watch-path") =>
        {
            tests
        }
        "rg" if args.len() >= 2
            && !args.iter().any(|a| a == "--pre" || a.starts_with("--pre=")) =>
        {
            Some(Profile::Search)
        }
        "grep" if args.len() >= 3 => Some(Profile::Search),
        "find"
            if !args.iter().any(|a| {
                matches!(
                    a.as_str(),
                    "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir" | "-fls"
                ) || a.starts_with("-fprint")
            }) =>
        {
            Some(Profile::Search)
        }
        "ls" if args[1..]
            .iter()
            .any(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('R')) =>
        {
            Some(Profile::Search)
        }
        // `-o FILE` writes the listing to a file.
        "tree" if !args.iter().any(|a| a.starts_with("-o")) => Some(Profile::Search),
        "git" => {
            // Options before the subcommand: `-p`/`--paginate` there starts a
            // pager and `-c` can name a program (`core.fsmonitor`,
            // `diff.external`), so only `--no-pager` and `-C DIR` are allowed.
            let mut rest = &args[1..];
            while let Some(first) = rest.first() {
                match first.as_str() {
                    "--no-pager" => rest = &rest[1..],
                    "-C" if rest.len() > 1 => rest = &rest[2..],
                    _ => break,
                }
            }
            let command = rest.first().map(String::as_str).unwrap_or("");
            // Options that run a program, start a pager or write a file.
            let unsafe_option = |a: &String| {
                matches!(
                    a.as_str(),
                    "--paginate" | "--ext-diff" | "--textconv" | "--interactive"
                ) || ["--output", "--open-files-in-pager"]
                    .iter()
                    .any(|o| a == o || a.starts_with(&format!("{o}=")))
                    || (command == "grep" && a.starts_with("-O"))
            };
            if rest.iter().any(unsafe_option) {
                return None;
            }
            match command {
                "diff" | "log" | "show" | "status" | "blame" | "grep" => Some(Profile::Git),
                // A listing of tracked paths, folded by directory like `find`.
                "ls-files" => Some(Profile::Search),
                _ => None,
            }
        }
        "docker" | "kubectl" if second == "logs" && !follows(args) => Some(Profile::Logs),
        "jq" if args.len() >= 3 => Some(Profile::Logs),
        "cat" | "nl" if args.len() == 2 && !second.starts_with('-') => Some(Profile::FileRead),
        "head" | "tail"
            if args.len() >= 2 && !args[args.len() - 1].starts_with('-') && !follows(args) =>
        {
            Some(Profile::FileRead)
        }
        "sed" if args.len() >= 4 && filter(args).is_some() => Some(Profile::FileRead),
        _ => None,
    }
}

/// Terms a command line names that its output is likely to contain: the
/// pattern of a search (`rg`, `grep`, `git grep`) and the filter of a test
/// run (`cargo test NAME`, `pytest -k EXPR`, `jest -t NAME`, `go test -run`).
/// Only literal text counts: a regular expression with operators names
/// nothing a line contains verbatim.
pub fn anchors(command: &str) -> Vec<String> {
    let Some(script) = crate::shell::parse(command) else {
        return Vec::new();
    };
    let mut terms: Vec<String> = Vec::new();
    for simple in script.commands() {
        for term in argv_anchors(&simple.argv) {
            if !terms.contains(&term) {
                terms.push(term);
            }
        }
    }
    terms
}

/// `anchors` of one argument vector; `sh -c SCRIPT` is read as its script.
pub fn argv_anchors(args: &[String]) -> Vec<String> {
    let Some(first) = args.first() else {
        return Vec::new();
    };
    if args.len() == 3 && matches!(name(first), "sh" | "bash") && args[1] == "-c" {
        return anchors(&args[2]);
    }
    let second = args.get(1).map(String::as_str).unwrap_or("");
    let found: Vec<String> = match name(first) {
        "rg" | "grep" | "egrep" | "fgrep" => search_patterns(&args[1..]),
        "git" => match args.iter().position(|a| a == "grep") {
            Some(at) => search_patterns(&args[at + 1..]),
            None => Vec::new(),
        },
        "cargo" if matches!(second, "test" | "nextest") => args[2..]
            .iter()
            .take_while(|a| *a != "--")
            .filter(|a| !a.starts_with('-') && *a != "run")
            .take(1)
            .cloned()
            .collect(),
        "go" if second == "test" => option_value(args, &["-run", "--run"]),
        _ if args.iter().any(|a| a == "-k") || args.iter().any(|a| a.starts_with("-k=")) => {
            // pytest -k "parse and not slow": the names, not the operators.
            option_value(args, &["-k"])
                .iter()
                .flat_map(|e| e.split(|c: char| c.is_whitespace() || "()".contains(c)))
                .filter(|w| !matches!(*w, "and" | "or" | "not" | ""))
                .map(str::to_owned)
                .collect()
        }
        _ => option_value(args, &["-t", "--testNamePattern", "--test-name-pattern"]),
    };
    found
        .into_iter()
        .flat_map(|p| literal(&p))
        .filter(|t| crate::session::ANCHOR_BYTES.contains(&t.len()))
        .collect()
}

/// The value of the first of these options: `-k EXPR` or `-k=EXPR`.
fn option_value(args: &[String], options: &[&str]) -> Vec<String> {
    for (i, arg) in args.iter().enumerate() {
        for option in options {
            if arg == option {
                return args.get(i + 1).cloned().into_iter().collect();
            }
            if let Some(value) = arg.strip_prefix(&format!("{option}=")) {
                return vec![value.to_owned()];
            }
        }
    }
    Vec::new()
}

/// The patterns of a `grep`/`rg` argument list: every `-e`/`--regexp`
/// value, else the first operand. Options known to take a value skip it.
fn search_patterns(args: &[String]) -> Vec<String> {
    const VALUED: &[&str] = &[
        "-A",
        "-B",
        "-C",
        "-m",
        "-g",
        "-t",
        "-T",
        "-f",
        "-j",
        "-M",
        "--glob",
        "--iglob",
        "--type",
        "--type-not",
        "--max-count",
        "--context",
        "--after-context",
        "--before-context",
        "--file",
        "--max-columns",
        "--threads",
        "--max-depth",
        "--include",
        "--exclude",
    ];
    let mut explicit = Vec::new();
    let mut operand = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            operand = operand.or(args.get(i + 1).cloned());
            break;
        } else if arg == "-e" || arg == "--regexp" {
            explicit.extend(args.get(i + 1).cloned());
            i += 1;
        } else if let Some(value) = arg.strip_prefix("--regexp=") {
            explicit.push(value.to_owned());
        } else if VALUED.contains(&arg) {
            i += 1;
        } else if !arg.starts_with('-') && operand.is_none() {
            operand = Some(arg.to_owned());
        }
        i += 1;
    }
    if explicit.is_empty() {
        operand.into_iter().collect()
    } else {
        explicit
    }
}

/// `anchors` of a search pattern given on its own (Claude Code's Grep tool).
pub fn pattern_anchors(pattern: &str) -> Vec<String> {
    literal(pattern)
        .into_iter()
        .filter(|t| crate::session::ANCHOR_BYTES.contains(&t.len()))
        .collect()
}

/// The literal alternatives of a pattern: itself when it has no regular
/// expression operator, its `|` branches when that is the only one, else
/// nothing. A `\.` or `\-` escape is read as the character.
fn literal(pattern: &str) -> Vec<String> {
    let pattern = pattern.replace("\\.", ".").replace("\\-", "-");
    if pattern.contains(['\\', '(', ')', '[', ']', '{', '}', '*', '+', '?', '^', '$']) {
        return Vec::new();
    }
    pattern
        .split('|')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A command after a pipe that only reads its input and arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Filter {
    /// It already bounds what reaches the agent (`head`, `tail`, `wc`,
    /// `grep -c`/`-l`/`-q`): wrapping the pipeline would add nothing.
    pub bounds_output: bool,
}

/// Whether a short-flag cluster (`-il`) or a long flag names one of these.
fn has_flag(args: &[String], short: &[char], long: &[&str]) -> bool {
    args.iter().skip(1).any(|a| {
        if let Some(name) = a.strip_prefix("--") {
            long.contains(&name.split('=').next().unwrap_or(""))
        } else {
            a.strip_prefix('-')
                .is_some_and(|cluster| cluster.chars().any(|c| short.contains(&c)))
        }
    })
}

/// `sed -n` with a script that only prints (`10,20p`, `$p`, `/re/p`).
fn sed_prints_only(args: &[String]) -> bool {
    let script = match args.get(1..) {
        Some([flag, script, files @ ..]) if flag == "-n" => files
            .iter()
            .all(|f| !f.starts_with('-'))
            .then_some(script.as_str()),
        _ => None,
    };
    script.is_some_and(|script| {
        let address =
            |a: &str| a == "$" || (!a.is_empty() && a.bytes().all(|b| b.is_ascii_digit()));
        let Some(body) = script.strip_suffix('p') else {
            return false;
        };
        match body.split_once(',') {
            Some((a, b)) => address(a) && address(b),
            None => {
                address(body)
                    || (body.len() >= 2
                        && body.starts_with('/')
                        && body.ends_with('/')
                        && !body[1..body.len() - 1].contains('/'))
            }
        }
    })
}

/// A recognized read-only filter for the right side of a pipe; None for
/// anything that can write, execute or wait (`tee`, `xargs`, `awk`,
/// `sed` other than printing, `sort -o`, `tail -f`).
pub fn filter(args: &[String]) -> Option<Filter> {
    let name = name(args.first()?);
    let bounded = Filter {
        bounds_output: true,
    };
    let open = Filter {
        bounds_output: false,
    };
    match name {
        "head" | "wc" => Some(bounded),
        "tail" if !has_flag(args, &['f', 'F'], &["follow", "retry"]) => Some(bounded),
        "grep" | "egrep" | "fgrep" | "rg"
            if !args.iter().any(|a| a == "--pre" || a.starts_with("--pre=")) =>
        {
            let counts = has_flag(
                args,
                &['c', 'l', 'L', 'q'],
                &[
                    "count",
                    "files-with-matches",
                    "files-without-match",
                    "quiet",
                    "silent",
                ],
            );
            Some(Filter {
                bounds_output: counts,
            })
        }
        // `-o` writes a file and `--compress-program` runs one.
        "sort" if !has_flag(args, &['o'], &["output", "compress-program"]) => Some(open),
        // A second operand is an output file; `-` names standard input.
        "uniq"
            if args
                .iter()
                .skip(1)
                .filter(|a| *a == "-" || !a.starts_with('-'))
                .count()
                <= 1 =>
        {
            Some(open)
        }
        "cut" | "tr" | "nl" | "cat" | "jq" => Some(open),
        "sed" if sed_prints_only(args) => Some(open),
        _ => None,
    }
}

/// Literal environment assignments the hook may keep in front of a command:
/// display, locale and test knobs. Variables that can load or run another
/// program (`LD_PRELOAD`, `GIT_EXTERNAL_DIFF`, `PAGER`, `RUSTC_WRAPPER`,
/// `NODE_OPTIONS`, `PYTEST_ADDOPTS`, `GOFLAGS`, ...) leave the command native.
pub fn safe_env(name: &str) -> bool {
    matches!(
        name,
        "CI" | "NO_COLOR"
            | "FORCE_COLOR"
            | "CLICOLOR"
            | "TERM"
            | "COLUMNS"
            | "LANG"
            | "TZ"
            | "RUST_BACKTRACE"
            | "RUST_LOG"
            | "RUST_TEST_THREADS"
            | "RUST_MIN_STACK"
            | "CARGO_TERM_COLOR"
            | "CARGO_INCREMENTAL"
            | "PYTHONUNBUFFERED"
            | "PYTHONDONTWRITEBYTECODE"
            | "PYTHONHASHSEED"
            | "NODE_ENV"
            | "GOOS"
            | "GOARCH"
            | "CGO_ENABLED"
    ) || name.starts_with("LC_")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(command: &str) -> Vec<String> {
        command.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn filters_are_read_only_and_say_whether_they_bound_output() {
        for (command, bounds) in [
            ("head -n 5", true),
            ("tail -20", true),
            ("wc -l", true),
            ("grep -c x", true),
            ("grep -il x", true),
            ("rg --count x", true),
            ("grep -v Compiling", false),
            ("rg -n x", false),
            ("sort -u", false),
            ("uniq -c", false),
            ("cut -d: -f1", false),
            ("jq .name", false),
            ("sed -n 10,20p", false),
            ("sed -n $p", false),
            ("sed -n /error/p", false),
        ] {
            assert_eq!(
                filter(&args(command)),
                Some(Filter {
                    bounds_output: bounds
                }),
                "{command}"
            );
        }
        for command in [
            "tail -f",
            "tail --follow=name",
            "tee log",
            "xargs rm",
            "awk 1",
            "sed s/a/b/",
            "sed -n 1p -i",
            "sed -n 1w",
            "sed -n /a/b/p",
            "sort -o out",
            "sort --output=out",
            "uniq in out",
            "rg --pre cat x",
            "less",
        ] {
            assert_eq!(filter(&args(command)), None, "{command}");
        }
    }

    #[test]
    fn anchors_are_the_literal_terms_a_command_names() {
        for (command, expected) in [
            ("rg -n parse_header src", vec!["parse_header"]),
            ("rg -g '*.rs' -C 3 HttpClient", vec!["HttpClient"]),
            (
                "grep -rn -e timeout -e retry_after .",
                vec!["timeout", "retry_after"],
            ),
            (
                "git grep -n 'connect|reconnect'",
                vec!["connect", "reconnect"],
            ),
            ("rg 'fn \\w+\\('", vec![]),
            ("rg 'config\\.yaml'", vec!["config.yaml"]),
            (
                "cargo test parses_headers -- --nocapture",
                vec!["parses_headers"],
            ),
            ("cargo test", vec![]),
            (
                "pytest -k 'login and not slow' tests",
                vec!["login", "slow"],
            ),
            (
                "npx jest -t 'renders the header'",
                vec!["renders the header"],
            ),
            ("go test ./... -run TestParse", vec!["TestParse"]),
            ("cd sub && rg needle_one", vec!["needle_one"]),
            ("ls -la", vec![]),
            ("rg ab", vec![]),
        ] {
            assert_eq!(anchors(command), expected, "{command}");
        }
        let wrapped: Vec<String> = ["/bin/sh", "-c", "rg -n parse_header src"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(argv_anchors(&wrapped), ["parse_header"]);
    }

    #[test]
    fn commands_recognized_since_v4_stay_native_before_it() {
        for command in [
            "npm install",
            "pip install requests",
            "git ls-files",
            "uv run --with pytest python -m pytest",
            "uv run python -m pytest",
        ] {
            assert!(profile_for(&args(command), true).is_some(), "{command}");
            assert_eq!(profile_for(&args(command), false), None, "{command}");
            assert_eq!(classify_for(command, false), None, "{command}");
        }
        for command in ["cargo test", "uv run pytest", "git log -p", "rg needle"] {
            assert_eq!(
                profile_for(&args(command), false),
                profile(&args(command)),
                "{command}"
            );
        }
    }

    #[test]
    fn only_display_locale_and_test_knobs_are_safe_assignments() {
        for name in [
            "CI",
            "RUST_BACKTRACE",
            "LC_ALL",
            "NO_COLOR",
            "PYTHONUNBUFFERED",
        ] {
            assert!(safe_env(name), "{name}");
        }
        for name in [
            "LD_PRELOAD",
            "DYLD_INSERT_LIBRARIES",
            "GIT_EXTERNAL_DIFF",
            "PAGER",
            "RUSTC_WRAPPER",
            "NODE_OPTIONS",
            "PYTEST_ADDOPTS",
            "GOFLAGS",
            "BASH_ENV",
            "PATH",
        ] {
            assert!(!safe_env(name), "{name}");
        }
    }
}
