//! The deliberately small noninteractive command grammar shared by integrations.
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    Tests,
    Search,
    Git,
    Text,
}

pub fn profile(args: &[String]) -> Option<Profile> {
    let name = Path::new(args.first()?).file_name()?.to_str()?;
    let second = args.get(1).map(String::as_str).unwrap_or("");
    if args.iter().any(|a| {
        a.contains("scopelet")
            || a.contains("rtk")
            || matches!(a.as_str(), "--watch" | "--pdb" | "--interactive")
            || a.starts_with("--watch=")
    }) {
        return None;
    }
    match name {
        "cargo" if matches!(second, "test" | "check" | "clippy" | "build") => Some(Profile::Tests),
        "python3" if second.ends_with(".py") && !args.iter().any(|a| a == "-i") => {
            Some(Profile::Tests)
        }
        "pytest" if !args.iter().any(|a| a == "-w") => Some(Profile::Tests),
        "npm" | "pnpm" | "yarn" if !args.iter().any(|a| matches!(a.as_str(), "-w" | "-i")) => {
            let script = if second == "run" {
                args.get(2).map(String::as_str).unwrap_or("")
            } else {
                second
            };
            matches!(script, "test" | "build" | "lint" | "typecheck").then_some(Profile::Tests)
        }
        "go" if second == "test" => Some(Profile::Tests),
        "node"
            if second == "--test"
                && !args
                    .iter()
                    .any(|a| a.starts_with("--inspect") || a == "-i" || a == "--watch-path") =>
        {
            Some(Profile::Tests)
        }
        "rg" if args.len() >= 2
            && !args.iter().any(|a| a == "--pre" || a.starts_with("--pre=")) =>
        {
            Some(Profile::Search)
        }
        "grep" if args.len() >= 3 => Some(Profile::Search),
        "git" => {
            let command = if second == "--no-pager" {
                args.get(2).map(String::as_str).unwrap_or("")
            } else {
                second
            };
            (matches!(command, "diff" | "log" | "show" | "status")
                && !args.iter().any(|a| {
                    matches!(
                        a.as_str(),
                        "--paginate" | "-p" | "--ext-diff" | "--textconv" | "--interactive"
                    )
                }))
            .then_some(Profile::Git)
        }
        "cat" if args.len() == 2 && !second.starts_with('-') => Some(Profile::Text),
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
    let name = Path::new(args.first()?).file_name()?.to_str()?;
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
        "sort" if !has_flag(args, &['o'], &["output"]) => Some(open),
        "uniq" if args.iter().skip(1).filter(|a| !a.starts_with('-')).count() <= 1 => Some(open),
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
            | "PYTHONWARNINGS"
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
