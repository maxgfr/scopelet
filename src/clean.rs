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
    if memchr::memchr2(0x1b, b'\r', body.as_bytes()).is_none() {
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
#[cfg(test)]
pub(crate) fn template(line: &str) -> String {
    let mut out = String::new();
    template_into(line, &mut out);
    out
}

/// `template` into a caller-owned buffer, so a scan over many lines allocates
/// once instead of once per line.
pub(crate) fn template_into(line: &str, out: &mut String) {
    out.clear();
    out.reserve(line.len());
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(width) = whitespace_at(line, i) {
            i += width;
            while let Some(width) = whitespace_at(line, i) {
                i += width;
            }
            out.push(' ');
        } else if bytes[i].is_ascii_alphanumeric() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_alphanumeric() {
                i += 1;
            }
            let word = &line[start..i];
            let digits = word.bytes().filter(u8::is_ascii_digit).count();
            if digits == word.len()
                || (word.len() >= 8 && digits > 0 && word.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                out.push('#');
            } else {
                // A trailing counter inside a name (`module12`, `shard3`) is
                // an index, not part of the name: `module12.test.js` and
                // `module13.test.js` are the same line to a reader.
                let stem = word.trim_end_matches(|c: char| c.is_ascii_digit());
                out.push_str(stem);
                if stem.len() < word.len() {
                    out.push('#');
                }
            }
        } else if bytes[i].is_ascii() {
            out.push(bytes[i] as char);
            i += 1;
        } else {
            let c = line[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
        }
    }
}

/// Width in bytes of the whitespace character starting at `i`, if any. ASCII
/// is decided from the byte; anything else is decoded so Unicode whitespace
/// (no-break space, ideographic space, ...) collapses exactly as before.
fn whitespace_at(line: &str, i: usize) -> Option<usize> {
    let byte = *line.as_bytes().get(i)?;
    if byte.is_ascii() {
        // The ASCII members of `char::is_whitespace`: tab through carriage
        // return (vertical tab and form feed included) and space.
        return matches!(byte, 0x09..=0x0D | b' ').then_some(1);
    }
    let c = line[i..].chars().next()?;
    c.is_whitespace().then_some(c.len_utf8())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The character-by-character template this byte scan replaced.
    fn reference_template(line: &str) -> String {
        let mut out = String::new();
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
                    || (word.len() >= 8
                        && digits > 0
                        && word.bytes().all(|b| b.is_ascii_hexdigit()))
                {
                    out.push('#');
                } else {
                    let stem = word.trim_end_matches(|c: char| c.is_ascii_digit());
                    out.push_str(stem);
                    if stem.len() < word.len() {
                        out.push('#');
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn byte_template_matches_the_character_template() {
        let cases = [
            "",
            "plain words here",
            "  leading\t\x0B\x0Cmixed \r spaces  ",
            "\x1c\x1d\x1e\x1f control bytes stay",
            "module12.test.js took 42 ms (0x00ff00aa1234)",
            "deadbeef 12345678 abc12345 a1 é1 1é",
            "no-break\u{a0}space and\u{3000}ideographic\u{2028}separator",
            "\u{85}next line\u{85}\u{85}twice",
            "émoji 🎉 then 日本語 words 3 times",
            "tab\tvt\x0Bff\x0Ccr\rnl\nend",
            "unicode digits ٣٤ are not ascii 34",
            "trailing counters shard3 build007 v2",
        ];
        for line in cases {
            let mut out = String::new();
            template_into(line, &mut out);
            assert_eq!(out, reference_template(line), "{line:?}");
        }
    }

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
