use std::path::PathBuf;

use trogon_sandbox_contract::{
    FileOp, FileOpErrorKind, FileOpResponse, FileParentCreation, FileRecursion, FileStat, FileWriteTruncation, LeaseId,
    SandboxUserName,
};

use crate::read_chunk::ReadChunk;
use crate::{ClientError, FILE_TRANSFER_CHUNK_BYTES, FileChecksum, SandboxClient, TransportError, transfer_payload};

/// Bytes one ranged read or write moves.
pub(crate) fn chunk_bytes() -> u64 {
    FILE_TRANSFER_CHUNK_BYTES.min(FileOp::MAX_READ_BYTES)
}

impl SandboxClient {
    /// SHA-256 of `[offset, offset + length)` of a sandbox file; `length` of `None` hashes to the end. Needs a
    /// provider that advertises directory transfer support.
    pub async fn file_checksum(
        &self,
        lease_id: &LeaseId,
        path: &str,
        offset: u64,
        length: Option<u64>,
        as_user: Option<SandboxUserName>,
    ) -> Result<FileChecksum, ClientError> {
        let op = FileOp::Checksum {
            path: path.to_string(),
            offset,
            length,
        };
        match self.file_op(lease_id, op, as_user).await? {
            FileOpResponse::Checksum {
                sha256,
                bytes,
                total_size,
            } => Ok(FileChecksum {
                sha256,
                bytes,
                total_size,
            }),
            other => Err(Self::unexpected_file_reply("checksum", other)),
        }
    }

    /// Reads a whole file of any size by ranged reads of [`FILE_TRANSFER_CHUNK_BYTES`]. The file's size is checked on
    /// every chunk, so a file that grows or shrinks mid transfer is detected instead of returned torn.
    /// `max_total_bytes` bounds memory use.
    pub async fn download_file(
        &self,
        lease_id: &LeaseId,
        path: &str,
        max_total_bytes: u64,
        as_user: Option<SandboxUserName>,
    ) -> Result<Vec<u8>, ClientError> {
        let mut out = Vec::new();
        let mut expected_total: Option<u64> = None;
        loop {
            let offset = out.len() as u64;
            let chunk = self
                .transfer_read(lease_id, path, offset, chunk_bytes(), as_user.as_ref())
                .await?;
            match expected_total {
                None => {
                    if chunk.total_size > max_total_bytes {
                        return Err(ClientError::FileOpFailed {
                            kind: FileOpErrorKind::TooLarge,
                            message: format!(
                                "{path} is {} bytes, over the {max_total_bytes} byte download limit",
                                chunk.total_size
                            ),
                        });
                    }
                    expected_total = Some(chunk.total_size);
                }
                Some(total) if total != chunk.total_size => {
                    return Err(TransportError::SizeChanged {
                        path: PathBuf::from(path),
                    }
                    .into());
                }
                Some(_) => {}
            }
            let empty = chunk.data.is_empty();
            out.extend_from_slice(&chunk.data);
            if chunk.end || out.len() as u64 >= chunk.total_size {
                return Ok(out);
            }
            if empty {
                return Err(TransportError::EmptyChunk {
                    path: PathBuf::from(path),
                }
                .into());
            }
        }
    }

    /// Writes a whole file of any size as ordered chunks of [`FILE_TRANSFER_CHUNK_BYTES`]: the first chunk truncates,
    /// the rest land at increasing offsets. Re-running the same upload yields the same file, so a failed transfer is
    /// safe to retry from the start.
    pub async fn upload_file(
        &self,
        lease_id: &LeaseId,
        path: &str,
        data: &[u8],
        mode: Option<u32>,
        as_user: Option<SandboxUserName>,
    ) -> Result<u64, ClientError> {
        let chunk = FILE_TRANSFER_CHUNK_BYTES as usize;
        let mut offset = 0usize;
        loop {
            let end = (offset + chunk).min(data.len());
            let first = offset == 0;
            let size = self
                .transfer_write(
                    lease_id,
                    path,
                    &data[offset..end],
                    offset as u64,
                    if first { mode } else { None },
                    as_user.as_ref(),
                )
                .await?;
            offset = end;
            if offset >= data.len() {
                return Ok(size);
            }
        }
    }

    /// Packs a file or directory tree into one tar stream. Trees over the archive limit fail with
    /// [`FileOpErrorKind::TooLarge`]; move those file by file instead.
    pub async fn download_archive(
        &self,
        lease_id: &LeaseId,
        path: &str,
        max_bytes: Option<u64>,
        as_user: Option<SandboxUserName>,
    ) -> Result<Vec<u8>, ClientError> {
        let op = FileOp::ArchiveGet {
            path: path.to_string(),
            max_bytes,
        };
        match self.file_op(lease_id, op, as_user).await? {
            FileOpResponse::Archive { data, .. } => Ok(transfer_payload::decode(&data)?),
            other => Err(Self::unexpected_file_reply("archive_get", other)),
        }
    }

    /// Unpacks a tar stream into the directory `path`. Returns the number of entries.
    pub async fn upload_archive(
        &self,
        lease_id: &LeaseId,
        path: &str,
        archive: &[u8],
        as_user: Option<SandboxUserName>,
    ) -> Result<u64, ClientError> {
        let op = FileOp::ArchivePut {
            path: path.to_string(),
            data: transfer_payload::encode(archive)?,
        };
        match self.file_op(lease_id, op, as_user).await? {
            FileOpResponse::Extracted { entries, .. } => Ok(entries),
            other => Err(Self::unexpected_file_reply("archive_put", other)),
        }
    }

    /// Reads up to `length` bytes at `offset`.
    pub(crate) async fn transfer_read(
        &self,
        lease_id: &LeaseId,
        path: &str,
        offset: u64,
        length: u64,
        as_user: Option<&SandboxUserName>,
    ) -> Result<ReadChunk, ClientError> {
        let op = FileOp::Read {
            path: path.to_string(),
            offset,
            length: Some(length),
            max_bytes: Some(length),
        };
        match self.file_op(lease_id, op, as_user.cloned()).await? {
            FileOpResponse::Content {
                data, total_size, eof, ..
            } => Ok(ReadChunk {
                data: transfer_payload::decode(&data)?,
                total_size,
                end: eof.reached_end(),
            }),
            other => Err(Self::unexpected_file_reply("read", other)),
        }
    }

    /// Writes `data` at `offset`. The write at offset zero truncates the file and applies `mode`. Returns the file's
    /// size afterwards.
    pub(crate) async fn transfer_write(
        &self,
        lease_id: &LeaseId,
        path: &str,
        data: &[u8],
        offset: u64,
        mode: Option<u32>,
        as_user: Option<&SandboxUserName>,
    ) -> Result<u64, ClientError> {
        let op = FileOp::Write {
            path: path.to_string(),
            data: transfer_payload::encode(data)?,
            offset,
            truncate: FileWriteTruncation::from(offset == 0),
            mode,
            create_parents: FileParentCreation::Create,
        };
        match self.file_op(lease_id, op, as_user.cloned()).await? {
            FileOpResponse::Written { size, .. } => Ok(size),
            other => Err(Self::unexpected_file_reply("write", other)),
        }
    }

    pub(crate) async fn transfer_stat(
        &self,
        lease_id: &LeaseId,
        path: &str,
        as_user: Option<&SandboxUserName>,
    ) -> Result<FileStat, ClientError> {
        let op = FileOp::Stat { path: path.to_string() };
        match self.file_op(lease_id, op, as_user.cloned()).await? {
            FileOpResponse::Stat { stat } => Ok(stat),
            other => Err(Self::unexpected_file_reply("stat", other)),
        }
    }

    /// Lists the tree under `path`. The flag is true when the listing hit the entry ceiling.
    pub(crate) async fn transfer_list(
        &self,
        lease_id: &LeaseId,
        path: &str,
        as_user: Option<&SandboxUserName>,
    ) -> Result<(Vec<FileStat>, bool), ClientError> {
        let op = FileOp::List {
            path: path.to_string(),
            recursive: FileRecursion::Recursive,
            max_entries: Some(FileOp::MAX_ENTRIES),
        };
        match self.file_op(lease_id, op, as_user.cloned()).await? {
            FileOpResponse::Listing { entries, truncated } => Ok((entries, truncated.is_truncated())),
            other => Err(Self::unexpected_file_reply("list", other)),
        }
    }

    pub(crate) async fn transfer_mkdir(
        &self,
        lease_id: &LeaseId,
        path: &str,
        as_user: Option<&SandboxUserName>,
    ) -> Result<(), ClientError> {
        let op = FileOp::Mkdir {
            path: path.to_string(),
            parents: FileParentCreation::Create,
        };
        match self.file_op(lease_id, op, as_user.cloned()).await? {
            FileOpResponse::Created { .. } => Ok(()),
            other => Err(Self::unexpected_file_reply("mkdir", other)),
        }
    }
}
