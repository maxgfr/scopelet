use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const MAX_INPUT: usize = 32 * 1024 * 1024;
pub const MAX_STORE_FILE: usize = 256 * 1024 * 1024;

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
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
        Ok(Self { root })
    }

    fn location(&self, id: &str) -> Result<PathBuf> {
        let (kind, hash) = id
            .split_once(':')
            .context("expected blob:<sha256> or artifact:<sha256>")?;
        ensure!(
            hash.len() == 64 && hash.bytes().all(|c| c.is_ascii_hexdigit()),
            "invalid content hash"
        );
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
        let mut temp = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
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
            if now
                .duration_since(item.metadata()?.modified()?)
                .unwrap_or_default()
                >= age
            {
                fs::remove_file(item.path())?;
                removed += 1;
            } else {
                // Fail closed on malformed surviving artifacts: do not discard their originals.
                let bytes =
                    self.get(&format!("artifact:{}", item.file_name().to_string_lossy()))?;
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
            let id = format!("blob:{}", item.file_name().to_string_lossy());
            if item.file_type()?.is_file()
                && !referenced.contains(&id)
                && now
                    .duration_since(item.metadata()?.modified()?)
                    .unwrap_or_default()
                    >= age
            {
                fs::remove_file(item.path())?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}
