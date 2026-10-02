//! A deliberately small POSIX shell subset for Codex's rewrite hook.
//!
//! The hook may only wrap a command whose meaning it fully understands, so
//! this grammar accepts what reads the same in `sh`, `bash` and `zsh` and
//! rejects everything else: no expansion (`$`, backticks, globs, `~`, braces,
//! history), no grouping, no input redirection, no background job, no
//! sequencing other than `&&`, `|| true` and pipes. Words may be single
//! quoted, double quoted (without `$`, backticks or backslashes inside) or
//! escaped with a backslash. The only redirections are `2>&1` and output or
//! error to `/dev/null`. A rejected command is left to the host untouched.

/// A redirection the grammar accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Redirect {
    /// `2>&1`
    ErrToOut,
    /// `>/dev/null` or `1>/dev/null`
    OutToNull,
    /// `2>/dev/null`
    ErrToNull,
}

/// One simple command: literal assignments, words and redirections.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Simple {
    pub env: Vec<(String, String)>,
    pub argv: Vec<String>,
    pub redirects: Vec<Redirect>,
}

/// How a pipeline is joined to the one before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Joint {
    First,
    And,
    /// `|| true`: the only accepted use of `||`.
    OrTrue,
}

/// Pipelines joined by `&&` (and a final `|| true`).
#[derive(Debug, PartialEq, Eq)]
pub struct Script {
    pub steps: Vec<(Joint, Vec<Simple>)>,
}

/// At most this many pipelines, as the AND-list support always allowed.
const MAX_STEPS: usize = 8;

#[derive(Debug, PartialEq)]
enum Token {
    /// A word after quote removal, and whether it is a literal assignment
    /// (an unquoted valid name followed by an unquoted `=`).
    Word(String, bool),
    And,
    Or,
    Pipe,
    Redirect(Redirect),
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn lex(command: &str) -> Option<Vec<Token>> {
    let chars: Vec<char> = command.chars().collect();
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut started = false;
    // Characters appended before any quote or escape: an assignment's name
    // and `=` must be unquoted, and a redirection's fd must be bare digits.
    let mut bare = 0usize;
    let mut quoted = false;
    // Ends the current word, if any; the next one starts unquoted.
    let finish = |tokens: &mut Vec<Token>,
                  word: &mut String,
                  started: &mut bool,
                  bare: &mut usize,
                  quoted: &mut bool| {
        if *started {
            let assignment = word[..*bare]
                .split_once('=')
                .is_some_and(|(name, _)| valid_name(name));
            tokens.push(Token::Word(std::mem::take(word), assignment));
            *started = false;
        }
        *bare = 0;
        *quoted = false;
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' => {
                finish(&mut tokens, &mut word, &mut started, &mut bare, &mut quoted);
                i += 1;
                continue;
            }
            '\'' => {
                let end = chars[i + 1..].iter().position(|&c| c == '\'')? + i + 1;
                word.extend(&chars[i + 1..end]);
                started = true;
                quoted = true;
                i = end + 1;
                continue;
            }
            '"' => {
                let end = chars[i + 1..].iter().position(|&c| c == '"')? + i + 1;
                let inner = &chars[i + 1..end];
                if inner
                    .iter()
                    .any(|c| matches!(c, '$' | '`' | '\\' | '\n' | '\r'))
                {
                    return None;
                }
                word.extend(inner);
                started = true;
                quoted = true;
                i = end + 1;
                continue;
            }
            '\\' => {
                let next = *chars.get(i + 1)?;
                if matches!(next, '\n' | '\r') {
                    return None;
                }
                word.push(next);
                started = true;
                quoted = true;
                i += 2;
                continue;
            }
            '&' => {
                if chars.get(i + 1) != Some(&'&') {
                    return None; // background job or `&>`
                }
                finish(&mut tokens, &mut word, &mut started, &mut bare, &mut quoted);
                tokens.push(Token::And);
                i += 2;
            }
            '|' => {
                finish(&mut tokens, &mut word, &mut started, &mut bare, &mut quoted);
                match chars.get(i + 1) {
                    Some('|') => {
                        tokens.push(Token::Or);
                        i += 2;
                    }
                    Some('&') => return None,
                    _ => {
                        tokens.push(Token::Pipe);
                        i += 1;
                    }
                }
            }
            '>' => {
                // An optional bare fd digit right before `>`.
                let fd = if !started {
                    1
                } else if !quoted && (word == "1" || word == "2") {
                    let fd = if word == "2" { 2 } else { 1 };
                    word.clear();
                    started = false;
                    bare = 0;
                    fd
                } else {
                    return None;
                };
                i += 1;
                let redirect = if chars.get(i) == Some(&'&') {
                    if fd != 2 || chars.get(i + 1) != Some(&'1') {
                        return None;
                    }
                    i += 2;
                    Redirect::ErrToOut
                } else {
                    while chars.get(i) == Some(&' ') {
                        i += 1;
                    }
                    let start = i;
                    while i < chars.len() && !matches!(chars[i], ' ' | '\t' | '&' | '|') {
                        i += 1;
                    }
                    if chars[start..i].iter().collect::<String>() != "/dev/null" {
                        return None;
                    }
                    if fd == 2 {
                        Redirect::ErrToNull
                    } else {
                        Redirect::OutToNull
                    }
                };
                if chars
                    .get(i)
                    .is_some_and(|&c| !matches!(c, ' ' | '\t' | '&' | '|'))
                {
                    return None;
                }
                tokens.push(Token::Redirect(redirect));
            }
            '\n' | '\r' | ';' | '<' | '(' | ')' | '{' | '}' | '$' | '`' | '*' | '?' | '[' | '~'
            | '!' => return None,
            '#' | '=' if !started => return None,
            c => {
                word.push(c);
                if !quoted {
                    bare = word.len();
                }
                started = true;
                i += 1;
            }
        }
    }
    finish(&mut tokens, &mut word, &mut started, &mut bare, &mut quoted);
    Some(tokens)
}

/// Parse a command in the accepted subset; None for anything else.
pub fn parse(command: &str) -> Option<Script> {
    let tokens = lex(command)?;
    let mut steps: Vec<(Joint, Vec<Simple>)> = vec![(Joint::First, vec![Simple::default()])];
    for token in tokens {
        let (_, pipeline) = steps.last_mut().unwrap();
        let simple = pipeline.last_mut().unwrap();
        match token {
            Token::Word(word, assignment) if assignment && simple.argv.is_empty() => {
                let (name, value) = word.split_once('=').unwrap();
                simple.env.push((name.into(), value.into()));
            }
            Token::Word(word, _) => simple.argv.push(word),
            Token::Redirect(redirect) => simple.redirects.push(redirect),
            Token::Pipe => {
                if simple.argv.is_empty() {
                    return None;
                }
                pipeline.push(Simple::default());
            }
            Token::And | Token::Or => {
                if simple.argv.is_empty() {
                    return None;
                }
                let joint = if token == Token::And {
                    Joint::And
                } else {
                    Joint::OrTrue
                };
                steps.push((joint, vec![Simple::default()]));
            }
        }
    }
    let last = steps.last().unwrap().1.last().unwrap();
    if last.argv.is_empty() || steps.len() > MAX_STEPS {
        return None;
    }
    // `|| true` only as the final step, exactly.
    for (i, (joint, pipeline)) in steps.iter().enumerate() {
        if *joint == Joint::OrTrue {
            let plain_true = pipeline.len() == 1
                && pipeline[0].argv == ["true"]
                && pipeline[0].env.is_empty()
                && pipeline[0].redirects.is_empty();
            if i + 1 != steps.len() || !plain_true {
                return None;
            }
        }
    }
    Some(Script { steps })
}

/// How an accepted command is executed by `run --auto`.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// One simple command: executed directly from its argv.
    Direct(Vec<String>),
    /// Simple commands joined by `&&`: rebuilt from quoted argv.
    AndList(Vec<Vec<String>>),
    /// Anything else in the subset: the original string through `/bin/sh -c`.
    Shell,
}

impl Script {
    /// Every simple command, in order.
    pub fn commands(&self) -> impl Iterator<Item = &Simple> {
        self.steps.iter().flat_map(|(_, pipeline)| pipeline)
    }

    pub fn plan(&self) -> Plan {
        let plain = |s: &Simple| s.env.is_empty() && s.redirects.is_empty();
        let simple_steps = self.steps.iter().all(|(joint, pipeline)| {
            *joint != Joint::OrTrue && pipeline.len() == 1 && plain(&pipeline[0])
        });
        if simple_steps && !self.commands().any(|s| s.argv[0] == "cd") {
            let mut argvs: Vec<Vec<String>> = self.commands().map(|s| s.argv.clone()).collect();
            if argvs.len() == 1 {
                return Plan::Direct(argvs.pop().unwrap());
            }
            return Plan::AndList(argvs);
        }
        Plan::Shell
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(script: &Script) -> Vec<Vec<&str>> {
        script
            .commands()
            .map(|s| s.argv.iter().map(String::as_str).collect())
            .collect()
    }

    #[test]
    fn accepted_commands_parse_to_their_words() {
        let cases: &[(&str, &[&[&str]])] = &[
            ("cargo test", &[&["cargo", "test"]]),
            (
                "cargo test -- --nocapture",
                &[&["cargo", "test", "--", "--nocapture"]],
            ),
            ("rg 'fn \\w+\\(' src", &[&["rg", "fn \\w+\\(", "src"]]),
            ("rg \"a b\" src", &[&["rg", "a b", "src"]]),
            (
                "grep -n foo\\ bar x.txt",
                &[&["grep", "-n", "foo bar", "x.txt"]],
            ),
            ("cargo test 2>&1", &[&["cargo", "test"]]),
            (
                "cargo test 2>&1 | tail -n 40",
                &[&["cargo", "test"], &["tail", "-n", "40"]],
            ),
            (
                "cd crates/core && cargo test",
                &[&["cd", "crates/core"], &["cargo", "test"]],
            ),
            ("npm test || true", &[&["npm", "test"], &["true"]]),
            ("go vet ./... 2>/dev/null", &[&["go", "vet", "./..."]]),
            ("git status >/dev/null", &[&["git", "status"]]),
            ("echo a=b", &[&["echo", "a=b"]]),
            (
                "rg -e 'a|b' --glob '*.rs'",
                &[&["rg", "-e", "a|b", "--glob", "*.rs"]],
            ),
            ("rg 'x#y' a#b", &[&["rg", "x#y", "a#b"]]),
        ];
        for (command, expected) in cases {
            let script = parse(command).unwrap_or_else(|| panic!("rejected: {command}"));
            let expected: Vec<Vec<&str>> = expected.iter().map(|w| w.to_vec()).collect();
            assert_eq!(words(&script), expected, "{command}");
        }
    }

    #[test]
    fn assignments_redirections_and_joints_are_recorded() {
        let script = parse("RUST_BACKTRACE=1 CI='yes' cargo test 2>&1 | tail -n 5").unwrap();
        let first = &script.steps[0].1[0];
        assert_eq!(
            first.env,
            [
                ("RUST_BACKTRACE".into(), "1".into()),
                ("CI".into(), "yes".into())
            ]
        );
        assert_eq!(first.redirects, [Redirect::ErrToOut]);
        // A quoted name or a later word is not an assignment.
        let script = parse("\"A=1\" cmd B=2").unwrap();
        assert!(script.steps[0].1[0].env.is_empty());
        assert_eq!(words(&script), [["A=1", "cmd", "B=2"]]);
        let script = parse("'A'=1 cmd").unwrap();
        assert!(script.steps[0].1[0].env.is_empty());
        assert_eq!(words(&script), [["A=1", "cmd"]]);
        let script = parse("make test || true").unwrap();
        assert_eq!(script.steps[1].0, Joint::OrTrue);
    }

    #[test]
    fn everything_outside_the_subset_is_rejected() {
        for command in [
            "",
            "   ",
            "echo $HOME",
            "echo \"$HOME\"",
            "echo `id`",
            "echo $(id)",
            "echo \"a\\\"b\"",
            "ls *.rs",
            "ls file?.txt",
            "ls [ab].txt",
            "ls ~/x",
            "echo {a,b}",
            "(cd x && make)",
            "! cargo test",
            "cargo test; rm -rf x",
            "cargo test &",
            "cargo test |& tail",
            "cargo test &> log",
            "cargo test > log",
            "cargo test >> /dev/null",
            "cargo test >| /dev/null",
            "cargo test 2>&2",
            "cargo test >&2",
            "cargo test 3>/dev/null",
            "cat < input",
            "cat <<EOF\nx\nEOF",
            "cargo test\nrm x",
            "cargo \\\ntest",
            "=cmd x",
            "#comment",
            "a || b",
            "a || true && b",
            "a || true | tail",
            "a && && b",
            "a | | b",
            "a |",
            "&& a",
            "FOO=1",
            "'unterminated",
            "\"unterminated",
            "a&&b&&c&&d&&e&&f&&g&&h&&i",
        ] {
            assert!(parse(command).is_none(), "accepted: {command:?}");
        }
    }

    #[test]
    fn plans_keep_direct_execution_where_they_can() {
        let plan = |c| parse(c).unwrap().plan();
        assert_eq!(
            plan("cargo test"),
            Plan::Direct(vec!["cargo".into(), "test".into()])
        );
        assert_eq!(
            plan("cargo fmt --check && cargo test"),
            Plan::AndList(vec![
                vec!["cargo".into(), "fmt".into(), "--check".into()],
                vec!["cargo".into(), "test".into()],
            ])
        );
        for shell in [
            "cargo test 2>&1",
            "FOO=1 cargo test",
            "cd x && cargo test",
            "cargo test | tail -n 3",
            "npm test || true",
        ] {
            assert_eq!(plan(shell), Plan::Shell, "{shell}");
        }
    }
}
