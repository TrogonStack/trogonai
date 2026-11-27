//! Verified, resumable transfer of single files and whole directory trees of any size.
//!
//! A tree moves as one ordered pass over its files. Each file moves in [`FILE_TRANSFER_CHUNK_BYTES`] ranged reads or
//! writes, so no message exceeds the bus payload limit and no file is bounded by the single-message archive ceiling.
//! Progress is the destination's own size: an interrupted transfer is resumed by running the same call again, which
//! hashes the bytes already there (`checksum` verb) and continues from that offset only when they are a prefix of the
//! source. A finished file is verified by a SHA-256 over the whole of both sides.

use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use trogon_sandbox_contract::{FileKind, FileOp, FileOpErrorKind, LeaseId, SandboxUserName};

use crate::file_outcome::FileOutcome;
use crate::file_transfer::chunk_bytes;
use crate::{ClientError, FILE_TRANSFER_CHUNK_BYTES, SandboxClient, TransferReport, TransportError};

fn io_error(path: &Path, source: std::io::Error) -> TransportError {
    TransportError::Local {
        path: path.to_path_buf(),
        source,
    }
}

fn hex(digest: impl AsRef<[u8]>) -> String {
    digest.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// Hashes the first `length` bytes of a local file.
async fn local_prefix_hash(path: &Path, length: u64) -> Result<String, TransportError> {
    let mut file = tokio::fs::File::open(path).await.map_err(|e| io_error(path, e))?;
    let mut hasher = Sha256::new();
    let mut remaining = length;
    let mut buffer = vec![0u8; 256 * 1024];
    while remaining > 0 {
        let want = remaining.min(buffer.len() as u64) as usize;
        let n = file.read(&mut buffer[..want]).await.map_err(|e| io_error(path, e))?;
        if n == 0 {
            return Err(TransportError::ShorterThanExpected {
                path: path.to_path_buf(),
            });
        }
        hasher.update(&buffer[..n]);
        remaining -= n as u64;
    }
    Ok(hex(hasher.finalize()))
}

/// Joins a listing-relative path onto a local root, refusing anything that could leave it.
fn confined_join(root: &Path, relative: &str) -> Result<PathBuf, TransportError> {
    let mut out = root.to_path_buf();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => {
                return Err(TransportError::PathEscapes {
                    path: PathBuf::from(relative),
                });
            }
        }
    }
    Ok(out)
}

fn join_remote(base: &str, relative: &str) -> String {
    let relative = relative.trim_start_matches('/');
    if base.ends_with('/') {
        format!("{base}{relative}")
    } else {
        format!("{base}/{relative}")
    }
}

#[cfg(unix)]
fn local_mode(meta: &std::fs::Metadata) -> Option<u32> {
    Some(std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o7777)
}

#[cfg(not(unix))]
fn local_mode(_meta: &std::fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
async fn apply_mode(path: &Path, mode: Option<u32>) -> Result<(), TransportError> {
    if let Some(mode) = mode {
        let permissions = std::os::unix::fs::PermissionsExt::from_mode(mode & 0o7777);
        tokio::fs::set_permissions(path, permissions)
            .await
            .map_err(|e| io_error(path, e))?;
    }
    Ok(())
}

#[cfg(not(unix))]
async fn apply_mode(_path: &Path, _mode: Option<u32>) -> Result<(), TransportError> {
    Ok(())
}

impl SandboxClient {
    /// Copies a sandbox file to `local`, resuming a partial copy already there and verifying the finished file against
    /// the sandbox's SHA-256.
    pub async fn download_file_to(
        &self,
        lease_id: &LeaseId,
        remote: &str,
        local: &Path,
        as_user: Option<SandboxUserName>,
    ) -> Result<TransferReport, ClientError> {
        let outcome = self.download_one(lease_id, remote, local, as_user.as_ref()).await?;
        let mut report = TransferReport::default();
        report.absorb(outcome);
        Ok(report)
    }

    async fn download_one(
        &self,
        lease_id: &LeaseId,
        remote: &str,
        local: &Path,
        as_user: Option<&SandboxUserName>,
    ) -> Result<FileOutcome, ClientError> {
        let remote_size = self.transfer_stat(lease_id, remote, as_user).await?.size;
        let mut start = match tokio::fs::metadata(local).await {
            Ok(meta) if meta.is_file() => meta.len(),
            Ok(_) => {
                return Err(TransportError::NotAFile {
                    path: local.to_path_buf(),
                }
                .into());
            }
            Err(_) => 0,
        };
        let mut resumed = false;
        if start > 0 {
            let reusable = start <= remote_size
                && self
                    .file_checksum(lease_id, remote, 0, Some(start), as_user.cloned())
                    .await?
                    .sha256
                    .as_str()
                    == local_prefix_hash(local, start).await?;
            if reusable {
                resumed = start < remote_size;
            } else {
                start = 0;
            }
        }
        if let Some(parent) = local.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| io_error(parent, e))?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(start == 0)
            .open(local)
            .await
            .map_err(|e| io_error(local, e))?;
        file.set_len(start).await.map_err(|e| io_error(local, e))?;
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(|e| io_error(local, e))?;
        let mut offset = start;
        let mut moved = 0u64;
        while offset < remote_size {
            let chunk = self
                .transfer_read(lease_id, remote, offset, chunk_bytes(), as_user)
                .await?;
            if chunk.total_size != remote_size {
                return Err(TransportError::SizeChanged {
                    path: PathBuf::from(remote),
                }
                .into());
            }
            if chunk.data.is_empty() {
                return Err(TransportError::EmptyChunk {
                    path: PathBuf::from(remote),
                }
                .into());
            }
            file.write_all(&chunk.data).await.map_err(|e| io_error(local, e))?;
            offset += chunk.data.len() as u64;
            moved += chunk.data.len() as u64;
        }
        file.flush().await.map_err(|e| io_error(local, e))?;
        drop(file);
        let remote_hash = self.file_checksum(lease_id, remote, 0, None, as_user.cloned()).await?;
        if remote_hash.total_size != remote_size
            || remote_hash.sha256.as_str() != local_prefix_hash(local, remote_size).await?
        {
            return Err(TransportError::IntegrityCheckFailed {
                path: local.to_path_buf(),
            }
            .into());
        }
        Ok(FileOutcome {
            bytes_transferred: moved,
            resumed,
            skipped: moved == 0 && remote_size > 0 && start == remote_size,
        })
    }

    /// Copies the local file `local` into the sandbox at `remote`, resuming a partial copy already there and verifying
    /// the finished file against a local SHA-256.
    pub async fn upload_file_from(
        &self,
        lease_id: &LeaseId,
        local: &Path,
        remote: &str,
        as_user: Option<SandboxUserName>,
    ) -> Result<TransferReport, ClientError> {
        let outcome = self.upload_one(lease_id, local, remote, as_user.as_ref()).await?;
        let mut report = TransferReport::default();
        report.absorb(outcome);
        Ok(report)
    }

    async fn upload_one(
        &self,
        lease_id: &LeaseId,
        local: &Path,
        remote: &str,
        as_user: Option<&SandboxUserName>,
    ) -> Result<FileOutcome, ClientError> {
        let meta = tokio::fs::metadata(local).await.map_err(|e| io_error(local, e))?;
        if !meta.is_file() {
            return Err(TransportError::NotAFile {
                path: local.to_path_buf(),
            }
            .into());
        }
        let local_size = meta.len();
        let mode = local_mode(&meta);

        let mut start = match self.transfer_stat(lease_id, remote, as_user).await {
            Ok(stat) if stat.kind == FileKind::File => stat.size,
            Ok(_) => {
                return Err(ClientError::FileOpFailed {
                    kind: FileOpErrorKind::IsADirectory,
                    message: format!("{remote} exists and is not a regular file"),
                });
            }
            Err(ClientError::FileNotFound(_)) => 0,
            Err(e) => return Err(e),
        };
        let mut resumed = false;
        if start > 0 {
            let reusable = start <= local_size
                && self
                    .file_checksum(lease_id, remote, 0, Some(start), as_user.cloned())
                    .await?
                    .sha256
                    .as_str()
                    == local_prefix_hash(local, start).await?;
            if reusable {
                resumed = start < local_size;
            } else {
                start = 0;
            }
        }
        let mut moved = 0u64;
        if start < local_size || local_size == 0 {
            let mut file = tokio::fs::File::open(local).await.map_err(|e| io_error(local, e))?;
            file.seek(std::io::SeekFrom::Start(start))
                .await
                .map_err(|e| io_error(local, e))?;
            let mut offset = start;
            let mut buffer = vec![0u8; FILE_TRANSFER_CHUNK_BYTES as usize];
            loop {
                let mut filled = 0usize;
                while filled < buffer.len() {
                    let n = file.read(&mut buffer[filled..]).await.map_err(|e| io_error(local, e))?;
                    if n == 0 {
                        break;
                    }
                    filled += n;
                }
                if filled == 0 && offset > 0 {
                    break;
                }
                let first = if offset == 0 { mode } else { None };
                self.transfer_write(lease_id, remote, &buffer[..filled], offset, first, as_user)
                    .await?;
                offset += filled as u64;
                moved += filled as u64;
                if filled < buffer.len() {
                    break;
                }
            }
        }
        let remote_hash = self.file_checksum(lease_id, remote, 0, None, as_user.cloned()).await?;
        if remote_hash.total_size != local_size
            || remote_hash.sha256.as_str() != local_prefix_hash(local, local_size).await?
        {
            return Err(TransportError::IntegrityCheckFailed {
                path: local.to_path_buf(),
            }
            .into());
        }
        Ok(FileOutcome {
            bytes_transferred: moved,
            resumed,
            skipped: moved == 0 && local_size > 0,
        })
    }

    /// Copies the sandbox directory `remote` into the local directory `local`, creating it. Files already complete
    /// are skipped and partial files are resumed, so rerunning a failed call finishes the tree. Every file is
    /// verified by SHA-256. Symlinks and special files are not copied. A tree of more than
    /// [`FileOp::MAX_ENTRIES`] entries is refused with `TooLarge`; move such a tree in several calls over its
    /// subdirectories.
    pub async fn download_dir(
        &self,
        lease_id: &LeaseId,
        remote: &str,
        local: &Path,
        as_user: Option<SandboxUserName>,
    ) -> Result<TransferReport, ClientError> {
        let (mut entries, truncated) = self.transfer_list(lease_id, remote, as_user.as_ref()).await?;
        if truncated {
            return Err(ClientError::FileOpFailed {
                kind: FileOpErrorKind::TooLarge,
                message: format!(
                    "{remote} holds more than {} entries; transfer its subdirectories separately",
                    FileOp::MAX_ENTRIES
                ),
            });
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        tokio::fs::create_dir_all(local).await.map_err(|e| io_error(local, e))?;
        let mut report = TransferReport::default();
        for entry in entries {
            let target = confined_join(local, &entry.path)?;
            match entry.kind {
                FileKind::Directory => {
                    tokio::fs::create_dir_all(&target)
                        .await
                        .map_err(|e| io_error(&target, e))?;
                    report.directories += 1;
                }
                FileKind::File => {
                    let outcome = self
                        .download_one(lease_id, &join_remote(remote, &entry.path), &target, as_user.as_ref())
                        .await?;
                    apply_mode(&target, entry.mode).await?;
                    report.absorb(outcome);
                }
                FileKind::Symlink | FileKind::Other => report.skipped_special += 1,
            }
        }
        Ok(report)
    }

    /// Copies the local directory `local` into the sandbox directory `remote`, creating it. Resumable and verified
    /// like [`Self::download_dir`]. Symlinks and special files are not copied.
    pub async fn upload_dir(
        &self,
        lease_id: &LeaseId,
        local: &Path,
        remote: &str,
        as_user: Option<SandboxUserName>,
    ) -> Result<TransferReport, ClientError> {
        let meta = tokio::fs::metadata(local).await.map_err(|e| io_error(local, e))?;
        if !meta.is_dir() {
            return Err(TransportError::NotADirectory {
                path: local.to_path_buf(),
            }
            .into());
        }
        self.transfer_mkdir(lease_id, remote, as_user.as_ref()).await?;
        let mut report = TransferReport {
            directories: 1,
            ..Default::default()
        };
        let mut pending = vec![(local.to_path_buf(), remote.to_string())];
        while let Some((dir, remote_dir)) = pending.pop() {
            let mut reader = tokio::fs::read_dir(&dir).await.map_err(|e| io_error(&dir, e))?;
            let mut children = Vec::new();
            while let Some(entry) = reader.next_entry().await.map_err(|e| io_error(&dir, e))? {
                children.push(entry);
            }
            children.sort_by_key(|e| e.file_name());
            for entry in children {
                let name = entry.file_name().to_string_lossy().into_owned();
                let child_remote = join_remote(&remote_dir, &name);
                let file_type = entry.file_type().await.map_err(|e| io_error(&entry.path(), e))?;
                if file_type.is_dir() {
                    self.transfer_mkdir(lease_id, &child_remote, as_user.as_ref()).await?;
                    report.directories += 1;
                    pending.push((entry.path(), child_remote));
                } else if file_type.is_file() {
                    let outcome = self
                        .upload_one(lease_id, &entry.path(), &child_remote, as_user.as_ref())
                        .await?;
                    report.absorb(outcome);
                } else {
                    report.skipped_special += 1;
                }
            }
        }
        Ok(report)
    }
}
