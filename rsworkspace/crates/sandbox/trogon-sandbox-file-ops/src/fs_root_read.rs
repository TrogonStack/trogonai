use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use sha2::{Digest, Sha256};
use trogon_sandbox_contract::{
    FileOp, FileOpErrorKind, FileOpResponse, FileReadEnd, FileRecursion, FileResultTruncation, FileStat,
};

use crate::content_hash::encode_digest;
use crate::fs_io::{open_read, sorted_entries, stat_of};
use crate::fs_root_core::Reply;
use crate::{FileOpError, FsRoot, payload};

impl FsRoot {
    pub(crate) fn list(&self, path: &str, recursion: FileRecursion, max_entries: Option<u32>) -> Reply<FileOpResponse> {
        let io = |e| FileOpError::from_io(e, path);
        let dir = self.resolve(path, true)?;
        let limit = max_entries
            .unwrap_or(FileOp::DEFAULT_MAX_ENTRIES)
            .clamp(1, FileOp::MAX_ENTRIES) as usize;
        let metadata = fs::metadata(&dir).map_err(io)?;
        if !metadata.is_dir() {
            return Err(FileOpError::failed(
                FileOpErrorKind::NotADirectory,
                format!("{path} is not a directory"),
            ));
        }
        let mut entries = Vec::new();
        let mut truncated = false;
        walk(&dir, "", recursion, limit, &mut entries, &mut truncated).map_err(io)?;
        Ok(FileOpResponse::Listing {
            entries,
            truncated: FileResultTruncation::from(truncated),
        })
    }

    pub(crate) fn stat(&self, path: &str) -> Reply<FileOpResponse> {
        let host = self.resolve(path, false)?;
        let metadata = fs::symlink_metadata(&host).map_err(|e| FileOpError::from_io(e, path))?;
        let name = host
            .file_name()
            .map_or_else(|| "/".to_string(), |name| name.to_string_lossy().into_owned());
        let mut stat = stat_of(&name, &name, &metadata, &host);
        stat.path = path.to_string();
        Ok(FileOpResponse::Stat { stat })
    }

    pub(crate) fn read(
        &self,
        path: &str,
        offset: u64,
        length: Option<u64>,
        max_bytes: Option<u64>,
    ) -> Reply<FileOpResponse> {
        let io = |e| FileOpError::from_io(e, path);
        let host = self.resolve(path, true)?;
        let mut file = open_read(&host).map_err(io)?;
        self.ensure_opened_inside(&file, path)?;
        let metadata = file.metadata().map_err(io)?;
        if metadata.is_dir() {
            return Err(directory_failure(path));
        }
        let total = metadata.len();
        let available = total.saturating_sub(offset);
        let wanted = length.map_or(available, |length| length.min(available));
        let cap = max_bytes.unwrap_or(FileOp::MAX_READ_BYTES).min(FileOp::MAX_READ_BYTES);
        let count = wanted.min(cap);
        let mut data = Vec::with_capacity(count as usize);
        if count > 0 {
            file.seek(SeekFrom::Start(offset)).map_err(io)?;
            file.take(count).read_to_end(&mut data).map_err(io)?;
        }
        let end = offset.saturating_add(data.len() as u64);
        Ok(FileOpResponse::Content {
            offset,
            total_size: total,
            eof: FileReadEnd::from(end >= total),
            truncated: FileResultTruncation::from(count < wanted),
            data: payload::encode(&data)?,
        })
    }

    pub(crate) fn checksum(&self, path: &str, offset: u64, length: Option<u64>) -> Reply<FileOpResponse> {
        let io = |e| FileOpError::from_io(e, path);
        let host = self.resolve(path, true)?;
        let mut file = open_read(&host).map_err(io)?;
        self.ensure_opened_inside(&file, path)?;
        let metadata = file.metadata().map_err(io)?;
        if metadata.is_dir() {
            return Err(directory_failure(path));
        }
        let total = metadata.len();
        let available = total.saturating_sub(offset);
        let wanted = length.map_or(available, |length| length.min(available));
        let mut hasher = Sha256::new();
        let mut hashed = 0u64;
        if wanted > 0 {
            file.seek(SeekFrom::Start(offset)).map_err(io)?;
            let mut reader = file.take(wanted);
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let read = reader.read(&mut buffer).map_err(io)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
                hashed += read as u64;
            }
        }
        Ok(FileOpResponse::Checksum {
            sha256: encode_digest(&hasher.finalize())?,
            bytes: hashed,
            total_size: total,
        })
    }
}

pub(crate) fn directory_failure(path: &str) -> FileOpError {
    FileOpError::failed(FileOpErrorKind::IsADirectory, format!("{path} is a directory"))
}

fn walk(
    dir: &Path,
    relative: &str,
    recursion: FileRecursion,
    limit: usize,
    out: &mut Vec<FileStat>,
    truncated: &mut bool,
) -> std::io::Result<()> {
    for entry in sorted_entries(dir)? {
        if out.len() >= limit {
            *truncated = true;
            return Ok(());
        }
        let metadata = entry.metadata()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };
        out.push(stat_of(&name, &path, &metadata, &entry.path()));
        if recursion.is_recursive() && metadata.is_dir() {
            walk(&entry.path(), &path, recursion, limit, out, truncated)?;
            if *truncated {
                return Ok(());
            }
        }
    }
    Ok(())
}
