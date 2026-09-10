//! Display-only cleaning of text lines for compact-v3.
//!
//! Stored bytes, line counts and absolute line labels never change: every
//! function here maps one source line to one displayed line, so `input:N`
//! keeps pointing at the same original line and `expand --find` keeps
//! searching the original bytes.
use std::borrow::Cow;

/// The line without its `\n` or `\r\n` terminator.
pub(crate) fn body(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

/// What a terminal would have left on screen for this line: terminal control
/// sequences removed and only the last carriage-return overwrite kept.
pub(crate) fn line_view(line: &str) -> Cow<'_, str> {
    let body = body(line);
    if !body.bytes().any(|b| b == 0x1b || b == b'\r') {
        return Cow::Borrowed(body);
    }
    let stripped = strip_escapes(body);
    // A progress bar rewrites the line with `\r`; the last non-empty segment is
    // the final state. Skipping empties keeps a trailing `\r` from blanking it.
    let last = stripped
        .rsplit('\r')
        .find(|segment| !segment.is_empty())
        .unwrap_or("");
    Cow::Owned(last.to_owned())
}

/// Remove ANSI CSI, OSC and two-byte escape sequences. Only ASCII bytes are
/// dropped, so the remainder is still the original UTF-8.
fn strip_escapes(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != 0x1b {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        i += 1;
        match bytes.get(i) {
            Some(b'[') => {
                i += 1;
                while i < bytes.len() && (0x30..=0x3f).contains(&bytes[i]) {
                    i += 1;
                }
                while i < bytes.len() && (0x20..=0x2f).contains(&bytes[i]) {
                    i += 1;
                }
                if i < bytes.len() && (0x40..=0x7e).contains(&bytes[i]) {
                    i += 1;
                }
            }
            Some(b']') => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == 0x07 {
                        i += 1;
                        break;
                    }
                    if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'\\') {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            Some(b) if (0x20..=0x2f).contains(b) => {
                while i < bytes.len() && (0x20..=0x2f).contains(&bytes[i]) {
                    i += 1;
                }
                if i < bytes.len() && (0x30..=0x7e).contains(&bytes[i]) {
                    i += 1;
                }
            }
            Some(b) if (0x40..=0x5f).contains(b) => i += 1,
            _ => {}
        }
    }
    String::from_utf8(out).expect("removing ASCII bytes preserves UTF-8")
}

/// Key under which lines that differ only by counters, identifiers or
/// alignment coincide: digit runs and hex runs of at least eight characters
/// (containing a digit) become `#`, whitespace runs become one space.
pub(crate) fn template(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if c.is_whitespace() {
            while chars.peek().is_some_and(|(_, c)| c.is_whitespace()) {
                chars.next();
            }
            out.push(' ');
        } else if c.is_ascii_alphanumeric() {
            let mut end = start + 1;
            while let Some(&(i, c)) = chars.peek() {
                if !c.is_ascii_alphanumeric() {
                    break;
                }
                end = i + 1;
                chars.next();
            }
            let word = &line[start..end];
            let digits = word.bytes().filter(u8::is_ascii_digit).count();
            if digits == word.len()
                || (word.len() >= 8 && digits > 0 && word.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                out.push('#');
            } else {
                out.push_str(word);
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_drop_terminators_escapes_and_overwritten_segments() {
        assert_eq!(line_view("plain\r\n"), "plain");
        assert_eq!(line_view("\x1b[31mFAILED\x1b[0m tests\n"), "FAILED tests");
        assert_eq!(line_view("10%\r50%\r100% done\n"), "100% done");
        assert_eq!(line_view("done\r\r\n"), "done");
        assert_eq!(line_view("\x1b]0;title\x07é\x1b[K\n"), "é");
        assert_eq!(line_view("\x1b(Bx"), "x");
        assert_eq!(line_view("\x1bMx"), "x");
    }

    #[test]
    fn templates_mask_counters_ids_and_alignment_only() {
        assert_eq!(
            template("progress 0042: unchanged  x"),
            "progress #: unchanged x"
        );
        assert_eq!(template("id=deadbeef01 name=added"), "id=# name=added");
        assert_eq!(template("error: case 7 failed"), "error: case # failed");
        assert_eq!(template("café 3"), "café #");
    }
}
