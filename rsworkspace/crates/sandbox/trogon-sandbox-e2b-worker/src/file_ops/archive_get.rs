use std::fs::File;

use trogon_sandbox_contract::{FileKind, FileOp, FileOpResponse, FileStat};
use trogon_sandbox_file_ops::FileOpError;

use super::collection_source::CollectionSource;
use super::constants::{
    DEFAULT_DIRECTORY_MODE, DEFAULT_FILE_MODE, EXPORT_MODE_MASK, MILLIS_PER_SECOND, TAR_TRAILER_BYTES,
};
use super::file_guest::FileGuest;
use super::payload::encode;
use super::staging::{download, staging_failure, with_staging};
use super::tar_size::tar_entry_bytes;

fn archived(entry: &FileStat) -> bool {
    matches!(entry.kind, FileKind::File | FileKind::Directory)
}

fn header_for(entry: &FileStat) -> tar::Header {
    let is_directory = entry.kind == FileKind::Directory;
    let mut header = tar::Header::new_gnu();
    header.set_mode(
        entry.mode.unwrap_or(if is_directory {
            DEFAULT_DIRECTORY_MODE
        } else {
            DEFAULT_FILE_MODE
        }) & EXPORT_MODE_MASK,
    );
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(
        entry
            .modified_unix_ms
            .and_then(|millis| u64::try_from(millis).ok())
            .unwrap_or(0)
            / MILLIS_PER_SECOND,
    );
    header.set_entry_type(if is_directory {
        tar::EntryType::Directory
    } else {
        tar::EntryType::Regular
    });
    header.set_size(if is_directory { 0 } else { entry.size });
    header.set_cksum();
    header
}

pub(super) async fn archive_get(
    guest: &FileGuest<'_>,
    path: &str,
    max_bytes: Option<u64>,
) -> Result<FileOpResponse, FileOpError> {
    with_staging(|directory| async move {
        let files = CollectionSource::load(guest, path).await?;
        if files.truncated {
            return Err(FileOpError::too_large("archive exceeds the entry limit"));
        }
        let cap = max_bytes
            .unwrap_or(FileOp::MAX_ARCHIVE_BYTES)
            .min(FileOp::MAX_ARCHIVE_BYTES);
        let (mut bytes, mut raw, mut entries) = (0u64, TAR_TRAILER_BYTES, 0u64);
        for entry in files.entries.iter().filter(|entry| archived(entry)) {
            entries += 1;
            let content = if entry.kind == FileKind::File { entry.size } else { 0 };
            raw = raw.saturating_add(tar_entry_bytes(&entry.path, content));
            bytes = bytes.saturating_add(content);
            if bytes > cap || raw > FileOp::MAX_ARCHIVE_BYTES {
                return Err(FileOpError::too_large("archive exceeds its content or tar byte limit"));
            }
        }
        for entry in files.entries.iter().filter(|entry| entry.kind == FileKind::File) {
            download(guest, &files.remote(entry), &directory.join(&entry.path), entry.size).await?;
        }
        let mut builder = tar::Builder::new(Vec::new());
        for entry in files.entries.iter().filter(|entry| archived(entry)) {
            let mut header = header_for(entry);
            if entry.kind == FileKind::Directory {
                builder
                    .append_data(&mut header, &entry.path, std::io::empty())
                    .map_err(staging_failure)?;
            } else {
                let content = File::open(directory.join(&entry.path)).map_err(staging_failure)?;
                builder
                    .append_data(&mut header, &entry.path, content)
                    .map_err(staging_failure)?;
            }
        }
        let data = builder.into_inner().map_err(staging_failure)?;
        Ok(FileOpResponse::Archive {
            data: encode(&data)?,
            entries,
            bytes,
        })
    })
    .await
}
