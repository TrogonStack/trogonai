use std::fs;
use std::io::{self, Cursor, Read};
use std::path::Path;

use trogon_sandbox_contract::{Base64Data, FileOp, FileOpResponse, FileWriteTruncation};

use crate::archive_summary::{MAX_ARCHIVE_ENTRIES, archive_relative};
use crate::fs_io::{lock, sorted_entries, write_at};
use crate::fs_root::Reply;
use crate::{ArchiveSummary, FileOpFailure, FsRoot, payload};

#[derive(Default)]
pub(crate) struct Tally {
    pub(crate) cap: u64,
    pub(crate) bytes: u64,
    pub(crate) entries: u64,
}

impl Tally {
    pub(crate) fn add_file(&mut self, len: u64, path: &str) -> Reply<()> {
        self.bytes = self.bytes.saturating_add(len);
        self.entries += 1;
        if self.bytes > self.cap {
            return Err(FileOpFailure::too_large(format!(
                "{path} holds more than {} bytes of file content; move it file by file with ranged reads and writes",
                self.cap
            )));
        }
        if self.entries > MAX_ARCHIVE_ENTRIES {
            return Err(FileOpFailure::too_large(format!(
                "{path} holds more than {MAX_ARCHIVE_ENTRIES} entries"
            )));
        }
        Ok(())
    }
}

impl FsRoot {
    pub(crate) fn archive_get(&self, path: &str, max_bytes: Option<u64>) -> Reply<FileOpResponse> {
        let io = |e: io::Error| FileOpFailure::from_io(e, path);
        let host = self.resolve(path, true)?;
        let metadata = fs::metadata(&host).map_err(io)?;
        let cap = max_bytes
            .unwrap_or(FileOp::MAX_ARCHIVE_BYTES)
            .min(FileOp::MAX_ARCHIVE_BYTES);
        let mut builder = tar::Builder::new(Vec::new());
        builder.follow_symlinks(false);
        let mut tally = Tally {
            cap,
            ..Tally::default()
        };
        if metadata.is_dir() {
            add_dir(&mut builder, &host, "", &mut tally)?;
        } else {
            let name = host
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            tally.add_file(metadata.len(), path)?;
            builder.append_path_with_name(&host, name).map_err(io)?;
        }
        let data = builder.into_inner().map_err(io)?;
        if data.len() as u64 > FileOp::MAX_ARCHIVE_BYTES {
            return Err(FileOpFailure::too_large(format!(
                "archive of {path} is {} bytes, over the {} byte limit",
                data.len(),
                FileOp::MAX_ARCHIVE_BYTES
            )));
        }
        Ok(FileOpResponse::Archive {
            data: payload::encode(&data)?,
            entries: tally.entries,
            bytes: tally.bytes,
        })
    }

    pub(crate) fn archive_put(&self, path: &str, data: &Base64Data) -> Reply<FileOpResponse> {
        let io = |e: io::Error| FileOpFailure::from_io(e, path);
        let data = payload::decode(data);
        ArchiveSummary::validate(&data)?;
        let dest = if path.is_empty() { "." } else { path };
        let root = self.resolve(dest, true)?;
        let _guard = lock();
        fs::create_dir_all(&root).map_err(io)?;
        let mut archive = tar::Archive::new(Cursor::new(data));
        let mut entries = 0u64;
        let mut bytes = 0u64;
        for entry in archive.entries().map_err(io)? {
            let mut entry = entry.map_err(io)?;
            let entry_path = entry.path().map_err(io)?.into_owned();
            let relative = archive_relative(&entry_path)?;
            if relative.is_empty() {
                continue;
            }
            let virtual_path = format!("{}/{relative}", dest.trim_end_matches('/'));
            let host = self.resolve(&virtual_path, true)?;
            let entry_io = |e: io::Error| FileOpFailure::from_io(e, &virtual_path);
            if entry.header().entry_type().is_dir() {
                fs::create_dir_all(&host).map_err(entry_io)?;
            } else {
                fs::create_dir_all(host.parent().unwrap_or(&self.root)).map_err(entry_io)?;
                let mode = entry.header().mode().ok().map(|mode| mode & 0o777);
                let mut content = Vec::new();
                entry.read_to_end(&mut content).map_err(entry_io)?;
                write_at(&host, &content, 0, FileWriteTruncation::Truncate, mode, &self.root).map_err(entry_io)?;
                bytes += content.len() as u64;
            }
            entries += 1;
        }
        Ok(FileOpResponse::Extracted { entries, bytes })
    }
}

fn add_dir(builder: &mut tar::Builder<Vec<u8>>, dir: &Path, relative: &str, tally: &mut Tally) -> Reply<()> {
    let io = |e: io::Error| FileOpFailure::from_io(e, relative);
    for entry in sorted_entries(dir).map_err(io)? {
        let metadata = entry.metadata().map_err(io)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = if relative.is_empty() {
            name
        } else {
            format!("{relative}/{name}")
        };
        if metadata.is_dir() {
            tally.entries += 1;
            builder.append_dir(&path, entry.path()).map_err(io)?;
            add_dir(builder, &entry.path(), &path, tally)?;
        } else if metadata.is_file() {
            tally.add_file(metadata.len(), &path)?;
            builder.append_path_with_name(entry.path(), &path).map_err(io)?;
        }
    }
    Ok(())
}
