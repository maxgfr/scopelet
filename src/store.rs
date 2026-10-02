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

/// Flush written data to the device before the rename publishes the item.
/// A full disk-cache flush (F_FULLFSYNC, which std uses for both sync calls on
/// Apple platforms) costs milliseconds per item; the temporary-plus-rename
/// publication and the hash check on every read already guarantee that an
/// interrupted write yields a missing or rejected item, never a wrong one.
fn flush(file: &fs::File) -> std::io::Result<()> {
    #[cfg(target_vendor = "apple")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: fsync on a valid open descriptor has no memory effects.
        match unsafe { libc::fsync(file.as_raw_fd()) } {
            0 => Ok(()),
            _ => Err(std::io::Error::last_os_error()),
        }
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        file.sync_data()
    }
}

/// The schema version of dataset artifact bytes, read before their body.
pub fn artifact_schema(bytes: &[u8]) -> Result<u32> {
    // Scopelet serializes the version first; anything else is parsed whole.
    for version in [1, 2] {
        if bytes.starts_with(format!("{{\"schema_version\":{version},").as_bytes()) {
            return Ok(version);
        }
    }
    #[derive(serde::Deserialize)]
    struct Probe {
        schema_version: u32,
    }
    Ok(serde_json::from_slice::<Probe>(bytes)
        .context("invalid dataset artifact")?
        .schema_version)
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
        if self.reuse(&path, temp.as_file().metadata()?.len())? {
            return Ok(id);
        }
        flush(temp.as_file())?;
        if let Err(error) = temp.persist_noclobber(&path) {
            if error.error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(error.error.into());
            }
            self.verify(&id)?;
        }
        Ok(id)
    }

    /// Reuse an already published item instead of re-reading it.
    ///
    /// The path is the item's content address, so a regular file of the right
    /// length is the item; every read still checks the hash, so a damaged file
    /// is rejected when it is used, never returned. Re-reading and re-hashing
    /// it here cost as much as the original capture (a second pass over a
    /// 32 MiB stream) for a check that reads already make. A file of the wrong
    /// length is an interrupted or truncated publication: drop it and let the
    /// caller publish the complete item again.
    fn reuse(&self, path: &Path, length: u64) -> Result<bool> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        ensure!(
            !metadata.file_type().is_symlink(),
            "cache item is a symlink"
        );
        ensure!(metadata.is_file(), "cache item is not a regular file");
        if metadata.len() != length {
            fs::remove_file(path)?;
            return Ok(false);
        }
        fs::File::open(path)?.set_modified(SystemTime::now())?;
        Ok(true)
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
        if self.reuse(&path, bytes.len() as u64)? {
            return Ok(id);
        }
        let mut temp = tempfile::Builder::new()
            .prefix(TEMP_PREFIX)
            .rand_bytes(12)
            .tempfile_in(path.parent().unwrap())?;
        temp.write_all(bytes)?;
        flush(temp.as_file())?;
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

    /// A verified dataset artifact with its records. Schema 1 stores them;
    /// schema 2 names the blob they are parsed from, which is read, verified
    /// and parsed again here. The dataset returned carries its records, so
    /// it is schema 1 whatever the artifact's schema: saving it (a query over
    /// it, a search of it) stores a self-contained artifact.
    pub fn dataset(&self, id: &str) -> Result<crate::model::Dataset> {
        self.load_dataset(id, true)
    }

    /// A verified dataset artifact without its records: metadata, notes and
    /// snapshots. A schema-2 artifact's blob is not read.
    pub fn dataset_head(&self, id: &str) -> Result<crate::model::Dataset> {
        self.load_dataset(id, false)
    }

    fn load_dataset(&self, id: &str, records: bool) -> Result<crate::model::Dataset> {
        use crate::model::{Dataset, DatasetRef};
        ensure!(
            id.starts_with("artifact:"),
            "expected an artifact reference"
        );
        let bytes = self.get(id)?;
        match artifact_schema(&bytes)? {
            1 => {
                let mut data: Dataset =
                    serde_json::from_slice(&bytes).context("invalid dataset artifact")?;
                if !records {
                    data.records.clear();
                }
                Ok(data)
            }
            2 => {
                let head: DatasetRef =
                    serde_json::from_slice(&bytes).context("invalid dataset artifact")?;
                let from = &head.records_from;
                let records = if records {
                    ensure!(
                        from.blob.starts_with("blob:"),
                        "artifact records must come from a blob"
                    );
                    let text = String::from_utf8(self.get(&from.blob)?)
                        .context("artifact records blob is not UTF-8")?;
                    crate::sources::Parsed::parse(text, from.format, &from.source)?
                        .records(&from.source, from.blob.clone())
                } else {
                    Vec::new()
                };
                Ok(Dataset {
                    schema_version: 1,
                    scan_complete: head.scan_complete,
                    examined: head.examined,
                    skipped: head.skipped,
                    notes: head.notes,
                    snapshots: head.snapshots,
                    records,
                })
            }
            version => bail!("unsupported artifact schema {version}"),
        }
    }

    /// Blobs a verified artifact refers to, without parsing its records again.
    pub fn references(&self, id: &str) -> Result<Vec<String>> {
        use crate::model::{Dataset, DatasetRef};
        let bytes = self.get(id)?;
        Ok(match artifact_schema(&bytes)? {
            1 => {
                let data: Dataset = serde_json::from_slice(&bytes)?;
                data.snapshots
                    .into_iter()
                    .map(|s| s.blob)
                    .chain(data.records.into_iter().filter_map(|r| r.blob))
                    .collect()
            }
            2 => {
                let data: DatasetRef = serde_json::from_slice(&bytes)?;
                data.snapshots
                    .into_iter()
                    .map(|s| s.blob)
                    .chain([data.records_from.blob])
                    .collect()
            }
            version => bail!("unsupported artifact schema {version}"),
        })
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
                referenced.extend(self.references(&format!("artifact:{name}"))?);
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
