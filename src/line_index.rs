//! Line ranges of a verified original, and the removal of the offset
//! indexes earlier versions cached for them.
//!
//! Until 1.2.6 a range recovery saved the byte offset of every line under
//! `line-index-v1/` and loaded it back on the next one. Loading validated
//! each offset against the original, which reads every newline of it: the
//! same pass that finds the lines without an index, plus a file to read,
//! parse and checksum. The lines are now found directly, and cleanup removes
//! the indexes left in existing caches.
use std::{fs, path::Path, time::Duration, time::SystemTime};

const DIRECTORY: &str = "line-index-v1";

/// Lines `start..=end` (1-based) of `text`, with their terminators, and how
/// many there are: fewer than asked when the text ends first, none when it
/// ends before `start`. One pass over the newlines, up to the last line
/// asked for.
pub(crate) fn slice(text: &str, start: usize, end: usize) -> (String, usize) {
    let bytes = text.as_bytes();
    // `line` is the number of the line that begins at `at`.
    let (mut line, mut at, mut from) = (1, 0, None);
    while at < bytes.len() && line <= end {
        if line == start {
            from = Some(at);
        }
        at = memchr::memchr(b'\n', &bytes[at..]).map_or(bytes.len(), |i| at + i + 1);
        line += 1;
    }
    match from {
        Some(from) => (text[from..at].to_owned(), line - start),
        None => (String::new(), 0),
    }
}

/// Remove the offset indexes of earlier versions, and aged temporaries of
/// their writes; the items and bytes removed. Anything else in the directory
/// is someone else's and stays.
pub(crate) fn clean(root: &Path, now: SystemTime, age: Duration) -> anyhow::Result<(usize, u64)> {
    let folder = root.join(DIRECTORY);
    let (mut items, mut bytes) = (0, 0);
    if !fs::symlink_metadata(&folder).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink()) {
        return Ok((items, bytes));
    }
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !entry.file_type()?.is_file() {
            continue;
        }
        if crate::store::content_hash_name(name) {
            let size = entry.metadata()?.len();
            fs::remove_file(entry.path())?;
            items += 1;
            bytes += size;
        } else if let Some(reaped) = crate::store::reap_temporary(&entry, name, now, age)? {
            items += 1;
            bytes += reaped;
        }
    }
    Ok((items, bytes))
}

#[cfg(test)]
mod tests {
    use super::slice;

    #[test]
    fn ranges_are_exact_with_and_without_a_final_newline() {
        let text = "a\r\nbé\r\nc\n";
        assert_eq!(slice(text, 1, 1), ("a\r\n".into(), 1));
        assert_eq!(slice(text, 2, 2), ("bé\r\n".into(), 1));
        assert_eq!(slice(text, 2, 3), ("bé\r\nc\n".into(), 2));
        assert_eq!(slice(text, 1, usize::MAX), (text.into(), 3));
        assert_eq!(slice(text, 3, 9), ("c\n".into(), 1));
        assert_eq!(slice(text, 4, 9), (String::new(), 0));
        assert_eq!(slice("x\ny", 2, 2), ("y".into(), 1));
        assert_eq!(slice("x\ny", 1, 5), ("x\ny".into(), 2));
        assert_eq!(slice("\n\n", 2, 2), ("\n".into(), 1));
        assert_eq!(slice("", 1, 1), (String::new(), 0));
    }
}
