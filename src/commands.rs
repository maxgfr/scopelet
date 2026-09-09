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
