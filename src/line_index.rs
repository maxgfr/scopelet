//! Disposable indexes bind byte offsets to a verified immutable blob.
use crate::store::{Store, digest, read_bounded};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{BufWriter, Write},
    path::Path,
    time::{Duration, SystemTime},
};

const MAX_LINES: usize = 100_000;
const DIRECTORY: &str = "line-index-v1";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    blob: String,
    offsets: Vec<usize>,
    checksum: String,
}

fn offsets(text: &str) -> Option<Vec<usize>> {
    let mut result = vec![0];
    for (i, byte) in text.bytes().enumerate() {
        if byte == b'\n' && i + 1 < text.len() {
            result.push(i + 1);
        }
        if result.len() > MAX_LINES {
            return None;
        }
    }
    result.push(text.len());
    Some(result)
}
fn checksum(blob: &str, offsets: &[usize]) -> String {
    digest(&serde_json::to_vec(&(blob, offsets)).unwrap())
}
fn load(path: &Path, blob: &str, text: &str) -> Option<Vec<usize>> {
    if fs::symlink_metadata(path).ok()?.file_type().is_symlink() {
        return None;
    }
    let index: Index = serde_json::from_slice(&read_bounded(path, 2 * 1024 * 1024).ok()?).ok()?;
    if index.blob != blob
        || index.offsets.len() < 2
        || index.offsets.len() > MAX_LINES + 1
        || index.offsets.first() != Some(&0)
        || index.offsets.last() != Some(&text.len())
        || index.checksum != checksum(blob, &index.offsets)
        || index.offsets.windows(2).any(|w| w[0] > w[1])
        || index.offsets[1..index.offsets.len() - 1]
            .iter()
            .any(|&i| i == 0 || i >= text.len() || text.as_bytes()[i - 1] != b'\n')
    {
        return None;
    }
    // A checksum stored beside offsets is not an authority for line numbers.
    // Validate every boundary against the verified original, including missing lines.
    let expected = std::iter::once(0)
        .chain(
            text.bytes()
                .enumerate()
                .filter_map(|(i, b)| (b == b'\n' && i + 1 < text.len()).then_some(i + 1)),
        )
        .chain(std::iter::once(text.len()));
    if !expected.eq(index.offsets.iter().copied()) {
        return None;
    }
    Some(index.offsets)
}

/// Caller supplies bytes returned by Store::get, which has checked their hash.
pub(crate) fn slice(
    store: &Store,
    blob: &str,
    text: &str,
    start: usize,
    end: usize,
) -> (String, usize) {
    let folder = store.root.join(DIRECTORY);
    let path = blob.strip_prefix("blob:").map(|hash| folder.join(hash));
    let safe_folder =
        fs::symlink_metadata(&folder).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink());
    let cached = if safe_folder {
        path.as_ref().and_then(|p| load(p, blob, text))
    } else {
        None
    };
    let index = cached.or_else(|| {
        let index = offsets(text)?;
        // Index storage is optional. It must never make recovery fail.
        let save = || -> anyhow::Result<()> {
            if !safe_folder {
                if fs::symlink_metadata(&folder).is_ok() {
                    return Ok(());
                }
                let mut builder = fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                builder.create(&folder)?;
            }
            crate::store::ignore_cached_evidence(&folder)?;
            let Some(path) = &path else {
                return Ok(());
            };
            let value = Index {
                blob: blob.into(),
                checksum: checksum(blob, &index),
                offsets: index.clone(),
            };
            let mut temp = tempfile::Builder::new()
                .prefix(".scopelet-write-")
                .rand_bytes(12)
                .tempfile_in(&folder)?;
            {
                let mut buffer = BufWriter::with_capacity(65536, &mut temp);
                serde_json::to_writer(&mut buffer, &value)?;
                buffer.flush()?;
            }
            temp.persist(path)?;
            Ok(())
        };
        let _ = save();
        Some(index)
    });
    if let Some(index) = index {
        let n = if text.is_empty() { 0 } else { index.len() - 1 };
        let lo = start.saturating_sub(1).min(n);
        let hi = end.min(n);
        if lo < hi {
            return (text[index[lo]..index[hi]].into(), hi - lo);
        }
        return (String::new(), 0);
    }
    let mut selected = String::new();
    let mut count = 0;
    for line in text
        .split_inclusive('\n')
        .skip(start.saturating_sub(1))
        .take(end.saturating_sub(start).saturating_add(1))
    {
        selected.push_str(line);
        count += 1;
    }
    (selected, count)
}

pub(crate) fn clean(root: &Path, now: SystemTime, age: Duration) -> anyhow::Result<usize> {
    let folder = root.join(DIRECTORY);
    if !fs::symlink_metadata(&folder).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink()) {
        return Ok(0);
    }
    let mut removed = 0;
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !entry.file_type()?.is_file() {
            continue;
        }
        if name.len() != 64 || !name.bytes().all(|c| c.is_ascii_hexdigit()) {
            removed += usize::from(crate::store::reap_temporary(&entry, name, now, age)?);
            continue;
        }
        if !root.join("blobs").join(name).exists()
            || now
                .duration_since(entry.metadata()?.modified()?)
                .unwrap_or_default()
                >= age
        {
            fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}
