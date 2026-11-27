//! Checkpoints of running leases, kept on this worker's host.
//!
//! A snapshot is three files: the VMM's device and vCPU state, the guest's memory, and a copy
//! of the root filesystem taken while the guest was paused. Nothing is pushed to a registry,
//! so only a lease placed on this worker can restore one.

use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use tokio::sync::{OwnedRwLockReadGuard, RwLock};
use tracing::warn;
use trogon_sandbox_contract::Digest;

use crate::config::SnapshotConfig;
use anyhow::{Context, ensure};
use tokio::io::AsyncReadExt;

mod constants;
use constants::{DIGEST_PREFIX, MEMORY, ROOTFS, STAGING_PREFIX, VM_STATE};

#[derive(Debug, thiserror::Error)]
pub enum SnapshotStoreError {
    #[error("snapshot store {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("could not mint a snapshot id: {0}")]
    Entropy(String),
}

/// Where the files of one snapshot live.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SnapshotFiles(PathBuf);

impl SnapshotFiles {
    pub fn vm_state(&self) -> PathBuf {
        self.0.join(VM_STATE)
    }

    pub fn memory(&self) -> PathBuf {
        self.0.join(MEMORY)
    }

    pub fn rootfs(&self) -> PathBuf {
        self.0.join(ROOTFS)
    }

    pub fn metadata(&self) -> PathBuf {
        self.0.join(super::METADATA_FILE)
    }

    pub async fn restore_metadata(
        &self,
        machine: super::RestoreMachine,
    ) -> anyhow::Result<Option<super::SnapshotMetadata>> {
        let inspect = |path: PathBuf| async move {
            let metadata = tokio::fs::symlink_metadata(&path).await?;
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "local snapshot payload must be a regular file"
            );
            Ok::<_, anyhow::Error>(metadata.len())
        };
        let memory_bytes = inspect(self.memory()).await?;
        ensure!(
            memory_bytes == u64::from(machine.memory.get()) * 1024 * 1024,
            "local snapshot memory configuration must match the lease"
        );
        let rootfs_bytes = inspect(self.rootfs()).await?;
        ensure!(
            rootfs_bytes > 0 && rootfs_bytes <= machine.rootfs_max_bytes.min(super::MAX_ROOTFS_BYTES),
            "local snapshot root filesystem exceeds the worker disk budget"
        );
        ensure!(
            inspect(self.vm_state()).await? <= super::MAX_STATE_BYTES,
            "local snapshot state exceeds 16 MiB"
        );
        let mut options = tokio::fs::OpenOptions::new();
        options.read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        let file = match options.open(self.metadata()).await {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let stat = file.metadata().await?;
        ensure!(stat.is_file(), "local snapshot metadata must be a regular file");
        ensure!(
            stat.len() <= super::MAX_METADATA_BYTES,
            "snapshot metadata exceeds 2 MiB"
        );
        let mut bytes = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            file.take(super::MAX_METADATA_BYTES + 1).read_to_end(&mut bytes),
        )
        .await
        .context("reading local snapshot metadata timed out")??;
        ensure!(
            bytes.len() as u64 <= super::MAX_METADATA_BYTES,
            "snapshot metadata exceeds 2 MiB"
        );
        let metadata: super::SnapshotMetadata = serde_json::from_slice(&bytes)?;
        metadata.validate()?;
        ensure!(
            metadata.memory_mib == machine.memory.get() && metadata.vcpus == machine.vcpus.get(),
            "snapshot memory and vCPU configuration must match the lease and worker"
        );
        ensure!(
            metadata.rootfs_bytes.is_none_or(|expected| expected == rootfs_bytes),
            "local snapshot root filesystem size differs from metadata"
        );
        Ok(Some(metadata))
    }

    fn all(&self) -> [PathBuf; 3] {
        [self.vm_state(), self.memory(), self.rootfs()]
    }
}

/// A snapshot being written, which no restore can see until it is committed.
#[derive(Debug)]
pub struct Staged {
    id: String,
    files: SnapshotFiles,
}

impl Staged {
    pub fn files(&self) -> &SnapshotFiles {
        &self.files
    }
}

/// A committed snapshot that stays on disk for as long as this is held.
#[derive(Debug)]
pub struct Held {
    files: SnapshotFiles,
    _eviction: OwnedRwLockReadGuard<()>,
}

impl Held {
    pub fn files(&self) -> &SnapshotFiles {
        &self.files
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct StoredSnapshot {
    pub digest: Digest,
    /// What the snapshot occupies on disk, not its files' apparent sizes.
    pub bytes: u64,
}

/// The snapshot id is random rather than a hash of the files: hashing the guest's memory
/// would add seconds to every checkpoint, and nothing off this host can fetch the files to
/// check a hash against. It is shaped as a digest because the wire names snapshots by one.
#[derive(Clone)]
pub struct SnapshotStore {
    config: SnapshotConfig,
    /// Restores read under it and commits evict under it, so a snapshot is never removed
    /// while a restore is still copying or mapping its files.
    eviction: Arc<RwLock<()>>,
}

impl SnapshotStore {
    pub fn new(config: SnapshotConfig) -> Self {
        Self {
            config,
            eviction: Arc::new(RwLock::new(())),
        }
    }

    fn dir(&self) -> &Path {
        self.config.dir.as_path()
    }

    /// Removes snapshots a previous run left half written.
    pub async fn discard_unfinished(&self) {
        let Ok(entries) = self.entries().await else {
            return;
        };
        for (name, path) in entries {
            if name.starts_with(STAGING_PREFIX) {
                remove(&path).await;
            }
        }
    }

    pub async fn stage(&self) -> Result<Staged, SnapshotStoreError> {
        let id = mint_id()?;
        let path = self.dir().join(format!("{STAGING_PREFIX}{id}"));
        tokio::fs::create_dir_all(&path)
            .await
            .map_err(|source| io_error(&path, source))?;
        tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|source| io_error(&path, source))?;
        Ok(Staged {
            id,
            files: SnapshotFiles(path),
        })
    }

    pub async fn discard(&self, staged: Staged) {
        remove(&staged.files.0).await;
    }

    pub async fn commit_captured(&self, snapshot: super::LocalSnapshot) -> Result<StoredSnapshot, SnapshotStoreError> {
        let store = self.clone();
        let error_path = self.dir().to_owned();
        tokio::spawn(async move {
            let staged = store.stage().await?;
            let result = async {
                let info = super::directory_info(snapshot.path())
                    .await
                    .map_err(|error| io_error(snapshot.path(), io::Error::other(error)))?;
                let reservation = crate::space::reserve(&staged.files.0, info.bytes)
                    .map_err(|error| io_error(&staged.files.0, io::Error::other(error)))?;
                for (name, target) in [
                    (super::STATE_FILE, staged.files.vm_state()),
                    (super::MEMORY_FILE, staged.files.memory()),
                    (super::ROOTFS_FILE, staged.files.rootfs()),
                    (super::METADATA_FILE, staged.files.metadata()),
                ] {
                    crate::disk::copy_private_reserved(&snapshot.path().join(name), &target, Arc::clone(&reservation))
                        .await
                        .map_err(|error| io_error(&target, io::Error::other(error)))?;
                }
                Ok::<_, SnapshotStoreError>(reservation)
            }
            .await;
            let reservation = match result {
                Ok(reservation) => reservation,
                Err(error) => {
                    store.discard(staged).await;
                    return Err(error);
                }
            };
            let staged_path = staged.files.0.clone();
            let result = store.commit(staged).await;
            if result.is_err() {
                remove(&staged_path).await;
            }
            drop(reservation);
            result
        })
        .await
        .map_err(|error| io_error(&error_path, io::Error::other(error)))?
    }

    /// Makes a staged snapshot restorable, then evicts the oldest beyond the configured count.
    pub async fn commit(&self, staged: Staged) -> Result<StoredSnapshot, SnapshotStoreError> {
        let mut bytes = 0;
        let mut payloads = staged.files.all().to_vec();
        match tokio::fs::symlink_metadata(staged.files.metadata()).await {
            Ok(_) => payloads.push(staged.files.metadata()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(&staged.files.metadata(), error)),
        }
        for file in payloads {
            let metadata = tokio::fs::symlink_metadata(&file)
                .await
                .map_err(|source| io_error(&file, source))?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(io_error(
                    &file,
                    io::Error::other("snapshot payload must be a regular file"),
                ));
            }
            tokio::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))
                .await
                .map_err(|source| io_error(&file, source))?;
            bytes += metadata.blocks() * 512;
        }
        let target = self.dir().join(&staged.id);
        let _evicting = self.eviction.write().await;
        tokio::fs::rename(&staged.files.0, &target)
            .await
            .map_err(|source| io_error(&target, source))?;
        self.evict_beyond_keep().await;
        Ok(StoredSnapshot {
            digest: digest_of(&staged.id)
                .ok_or_else(|| io_error(&target, std::io::Error::other("a minted id is not a digest")))?,
            bytes,
        })
    }

    /// The snapshot named by `digest`, held on disk until the returned guard is dropped.
    pub async fn hold(&self, digest: &Digest) -> Option<Held> {
        let id = snapshot_id(digest.as_str())?;
        let guard = Arc::clone(&self.eviction).read_owned().await;
        let files = SnapshotFiles(self.dir().join(id));
        for file in files.all() {
            if !tokio::fs::symlink_metadata(&file)
                .await
                .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
            {
                return None;
            }
        }
        match tokio::fs::symlink_metadata(files.metadata()).await {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            _ => return None,
        }
        Some(Held {
            files,
            _eviction: guard,
        })
    }

    /// Every snapshot this host can restore, newest first.
    pub async fn digests(&self) -> Vec<Digest> {
        self.committed()
            .await
            .into_iter()
            .map(|(digest, _, _)| digest)
            .collect()
    }

    async fn evict_beyond_keep(&self) {
        for (digest, path, _) in self.committed().await.into_iter().skip(self.config.keep.get()) {
            remove(&path).await;
            tracing::info!(digest = digest.as_str(), "oldest snapshot evicted");
        }
    }

    async fn committed(&self) -> Vec<(Digest, PathBuf, SystemTime)> {
        let Ok(entries) = self.entries().await else {
            return Vec::new();
        };
        let mut committed = Vec::new();
        for (name, path) in entries {
            let Some(digest) = digest_of(&name) else {
                continue;
            };
            let taken = tokio::fs::metadata(path.join(VM_STATE))
                .await
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            committed.push((digest, path, taken));
        }
        committed.sort_by_key(|entry| std::cmp::Reverse(entry.2));
        committed
    }

    async fn entries(&self) -> io::Result<Vec<(String, PathBuf)>> {
        let mut entries = match tokio::fs::read_dir(self.dir()).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                warn!(dir = %self.config.dir, error = %e, "could not list the snapshot dir");
                return Err(e);
            }
        };
        let mut found = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir()
                && let Some(name) = entry.file_name().to_str()
            {
                found.push((name.to_string(), entry.path()));
            }
        }
        Ok(found)
    }
}

fn mint_id() -> Result<String, SnapshotStoreError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| SnapshotStoreError::Entropy(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn snapshot_id(digest: &str) -> Option<&str> {
    let id = digest.strip_prefix(DIGEST_PREFIX)?;
    let hex = id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    (id.len() == 64 && hex).then_some(id)
}

fn digest_of(id: &str) -> Option<Digest> {
    Digest::new(format!("{DIGEST_PREFIX}{id}")).ok()
}

async fn remove(path: &Path) {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => warn!(path = %path.display(), error = %e, "could not remove a snapshot"),
    }
}

fn io_error(path: &Path, source: io::Error) -> SnapshotStoreError {
    SnapshotStoreError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests;
