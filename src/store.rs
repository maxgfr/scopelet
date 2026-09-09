use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const MAX_INPUT: usize = 32 * 1024 * 1024;
pub const MAX_STORE_FILE: usize = 256 * 1024 * 1024;
const TEMP_PREFIX: &str = ".scopelet-write-";

struct HashWriter<W> {
    inner: W,
    hash: Sha256,
    len: usize,
}
impl<W: Write> Write for HashWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_STORE_FILE.saturating_sub(self.len) {
            return Err(std::io::Error::other("artifact exceeds storage item limit"));
        }
        let n = self.inner.write(bytes)?;
        self.hash.update(&bytes[..n]);
        self.len += n;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn hash_json(value: &impl serde::Serialize, output: impl Write) -> Result<String> {
    let mut writer = HashWriter {
        inner: output,
        hash: Sha256::new(),
        len: 0,
    };
    {
        let mut buffer = BufWriter::with_capacity(65536, &mut writer);
        serde_json::to_writer(&mut buffer, value)?;
        buffer.flush()?;
    }
    Ok(format!("{:x}", writer.hash.finalize()))
}

/// Keep saved observations out of ordinary repository searches and Git staging.
/// Each marker is local to a storage directory; never change existing rules.
pub(crate) fn ignore_cached_evidence(directory: &Path) -> Result<()> {
    for name in [".ignore", ".gitignore"] {
        let path = directory.join(name);
        if fs::symlink_metadata(&path).is_ok() {
            continue;
        }
        let marker = tempfile::Builder::new()
            .prefix(TEMP_PREFIX)
            .rand_bytes(12)
            .tempfile_in(directory);
        let mut marker = match marker {
            Ok(marker) => marker,
            // Existing read-only caches must remain readable during migration.
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        marker.write_all(b"*\n")?;
        if let Err(error) = marker.persist_noclobber(&path)
            && error.error.kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(error.error).context("create cache search-exclusion marker");
        }
    }
    Ok(())
}

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Cache items are named by their content hash; anything else is not ours to read.
fn content_hash_name(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|c| c.is_ascii_hexdigit())
}

/// Discard an aged leftover from a write that was killed before it persisted.
pub(crate) fn reap_temporary(
    item: &fs::DirEntry,
    name: &str,
    now: SystemTime,
    age: Duration,
) -> Result<bool> {
    let suffix = name.strip_prefix(TEMP_PREFIX).unwrap_or("");
    if suffix.len() != 12
        || !suffix.bytes().all(|c| c.is_ascii_alphanumeric())
        || now
            .duration_since(item.metadata()?.modified()?)
            .unwrap_or_default()
            < age
    {
        return Ok(false);
    }
    fs::remove_file(item.path())?;
    Ok(true)
}

pub fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let file = fs::File::open(path).with_context(|| format!("read {}", path.display()))?;
    ensure!(
        file.metadata()?.is_file(),
        "not a regular file: {}",
        path.display()
    );
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= limit,
        "input exceeds {limit} bytes: {}",
        path.display()
    );
    Ok(bytes)
}

#[derive(Clone)]
pub struct Store {
    pub root: PathBuf,
}

impl Store {
    /// Serialize once to a bounded, hashed temporary; publish only a complete item.
    pub fn put_json(&self, value: &impl serde::Serialize) -> Result<String> {
        let temporary = tempfile::Builder::new()
            .prefix(TEMP_PREFIX)
            .rand_bytes(12)
            .tempfile_in(self.root.join("artifacts"));
        let mut temp = match temporary {
            Ok(temp) => temp,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
                ) =>
            {
                // Existing artifacts remain reusable in a read-only directory.
                let id = format!("artifact:{}", hash_json(value, std::io::sink())?);
                self.verify(&id)?;
                self.touch(&id)?;
                return Ok(id);
            }
            Err(error) => return Err(error.into()),
        };
        let hash = hash_json(value, temp.as_file_mut())?;
        let id = format!("artifact:{hash}");
        let path = self.location(&id)?;
        if path.exists() {
            self.verify(&id)?;
            self.touch(&id)?;
            return Ok(id);
        }
        temp.as_file().sync_all()?;
        if let Err(error) = temp.persist_noclobber(&path) {
            if error.error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(error.error.into());
            }
            self.verify(&id)?;
        }
        Ok(id)
    }

    fn verify(&self, id: &str) -> Result<()> {
        let path = self.location(id)?;
        ensure!(
            !fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "cache item is a symlink"
        );
        let file = fs::File::open(path)?;
        ensure!(
            file.metadata()?.is_file(),
            "cache item is not a regular file"
        );
        let mut reader = file.take(MAX_STORE_FILE as u64 + 1);
        let mut hash = Sha256::new();
        let mut count = 0;
        let mut buf = [0; 65536];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            count += n;
            ensure!(count <= MAX_STORE_FILE, "stored item exceeds limit");
            hash.update(&buf[..n]);
        }
        ensure!(
            id.ends_with(&format!("{:x}", hash.finalize())),
            "cache integrity mismatch for {id}"
        );
        Ok(())
    }
    pub fn open(path: Option<PathBuf>) -> Result<Self> {
        let root = path
            .or_else(|| std::env::var_os("SCOPELET_CACHE_DIR").map(PathBuf::from))
            .unwrap_or_else(|| {
                let base = std::env::var_os("XDG_CACHE_HOME")
                    .map(PathBuf::from)
                    .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
                    .unwrap_or_else(std::env::temp_dir);
                base.join("scopelet")
            });
        for dir in [&root, &root.join("blobs"), &root.join("artifacts")] {
            if let Ok(meta) = fs::symlink_metadata(dir) {
                ensure!(
                    !meta.file_type().is_symlink(),
                    "cache directory must not be a symlink"
                );
            }
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(dir)?;
        }
        for directory in [root.join("blobs"), root.join("artifacts")] {
            ignore_cached_evidence(&directory)?;
        }
        Ok(Self { root })
    }

    fn location(&self, id: &str) -> Result<PathBuf> {
        let (kind, hash) = id
            .split_once(':')
            .context("expected blob:<sha256> or artifact:<sha256>")?;
        ensure!(content_hash_name(hash), "invalid content hash");
        let folder = match kind {
            "blob" => "blobs",
            "artifact" => "artifacts",
            _ => bail!("unknown reference type"),
        };
        Ok(self.root.join(folder).join(hash))
    }

    pub fn put(&self, kind: &str, bytes: &[u8]) -> Result<String> {
        ensure!(
            bytes.len() <= MAX_STORE_FILE,
            "artifact exceeds storage item limit"
        );
        let id = format!("{kind}:{}", digest(bytes));
        let path = self.location(&id)?;
        if path.exists() {
            ensure!(self.get(&id)? == bytes, "cache integrity mismatch");
            fs::File::open(&path)?.set_modified(SystemTime::now())?;
            return Ok(id);
        }
        let mut temp = tempfile::Builder::new()
            .prefix(TEMP_PREFIX)
            .rand_bytes(12)
            .tempfile_in(path.parent().unwrap())?;
        temp.write_all(bytes)?;
        temp.as_file().sync_all()?;
        if let Err(error) = temp.persist_noclobber(&path) {
            if error.error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(error.error.into());
            }
            ensure!(
                self.get(&id)? == bytes,
                "concurrent cache integrity mismatch"
            );
        }
        Ok(id)
    }

    pub fn get(&self, id: &str) -> Result<Vec<u8>> {
        let path = self.location(id)?;
        ensure!(
            !fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "cache item is a symlink"
        );
        let bytes = read_bounded(&path, MAX_STORE_FILE)?;
        ensure!(
            id.ends_with(&digest(&bytes)),
            "cache integrity mismatch for {id}"
        );
        Ok(bytes)
    }

    /// Mark an item as used so age-based cleanup does not drop it mid-session.
    pub fn touch(&self, id: &str) -> Result<()> {
        fs::File::open(self.location(id)?)?.set_modified(SystemTime::now())?;
        Ok(())
    }

    pub fn clean(&self, older_days: u64) -> Result<usize> {
        let age = Duration::from_secs(older_days.saturating_mul(86400));
        let now = SystemTime::now();
        let mut removed = 0;
        let mut referenced = std::collections::BTreeSet::new();
        for item in fs::read_dir(self.root.join("artifacts"))? {
            let item = item?;
            if !item.file_type()?.is_file() {
                continue;
            }
            let name = item.file_name().to_string_lossy().into_owned();
            if !content_hash_name(&name) {
                // Foreign files are never cache items: leave them where they are.
                removed += usize::from(reap_temporary(&item, &name, now, age)?);
                continue;
            }
            if now
                .duration_since(item.metadata()?.modified()?)
                .unwrap_or_default()
                >= age
            {
                fs::remove_file(item.path())?;
                removed += 1;
            } else {
                // Fail closed on malformed surviving artifacts: do not discard their originals.
                let bytes = self.get(&format!("artifact:{name}"))?;
                let data: crate::model::Dataset = serde_json::from_slice(&bytes)?;
                for snapshot in data.snapshots {
                    referenced.insert(snapshot.blob);
                }
                for record in data.records {
                    if let Some(blob) = record.blob {
                        referenced.insert(blob);
                    }
                }
            }
        }
        for item in fs::read_dir(self.root.join("blobs"))? {
            let item = item?;
            let name = item.file_name().to_string_lossy().into_owned();
            if !item.file_type()?.is_file() {
                continue;
            }
            if !content_hash_name(&name) {
                removed += usize::from(reap_temporary(&item, &name, now, age)?);
                continue;
            }
            let id = format!("blob:{name}");
            if !referenced.contains(&id)
                && now
                    .duration_since(item.metadata()?.modified()?)
                    .unwrap_or_default()
                    >= age
            {
                fs::remove_file(item.path())?;
                removed += 1;
            }
        }
        removed += crate::line_index::clean(&self.root, now, age)?;
        Ok(removed)
    }
}
