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
        }
    }
    /// Compact-v3 byte target for this output.
    pub fn budget(self) -> usize {
        match self {
            Self::Tests => 4 * 1024,
            Self::Search | Self::Git | Self::Logs => 8 * 1024,
            Self::FileRead => 16 * 1024,
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
    crate::shell::parse(command)?
        .commands()
        .find_map(|simple| profile(&simple.argv))
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
        Some("vitest") => second == "run",
        Some("prettier") => args.iter().any(|a| a == "--check"),
        Some("playwright") => second == "test",
        _ => false,
    }
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
    match name {
        "cargo" => match second {
            "test" | "check" | "clippy" | "build" | "nextest" | "doc" => tests,
            "fmt" if has("--check") => tests,
            _ => None,
        },
        "python" | "python3" => {
            if has("-i") {
                None
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
            let script = if second == "run" {
                args.get(2).map(String::as_str).unwrap_or("")
            } else {
                second
            };
            matches!(script, "test" | "build" | "lint" | "typecheck").then_some(Profile::Tests)
        }
        "npx" | "bunx" => known_tool(&args[1..]).then_some(Profile::Tests),
        "uv" if second == "run" => known_tool(&args[2..]).then_some(Profile::Tests),
        "bun" if second == "test" => tests,
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
            (matches!(
                command,
                "diff" | "log" | "show" | "status" | "blame" | "grep"
            ) && !rest.iter().any(unsafe_option))
            .then_some(Profile::Git)
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
