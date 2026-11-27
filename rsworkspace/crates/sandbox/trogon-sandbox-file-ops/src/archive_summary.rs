use std::io::Cursor;
use std::path::{Component, Path};

use trogon_sandbox_contract::FileOp;

use crate::FileOpError;
use crate::constants::MAX_ARCHIVE_ENTRIES;

/// What a vetted tar stream holds.
///
/// A summary exists only for a stream that holds plain files and directories at relative paths and
/// fits the archive limits. Links and device nodes are refused because a link could point out of
/// the destination.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ArchiveSummary {
    entries: u64,
    bytes: u64,
}

impl ArchiveSummary {
    pub fn validate(data: &[u8]) -> Result<Self, FileOpError> {
        if data.len() as u64 > FileOp::MAX_ARCHIVE_BYTES {
            return Err(FileOpError::too_large(format!(
                "archive is {} bytes, over the {} byte limit",
                data.len(),
                FileOp::MAX_ARCHIVE_BYTES
            )));
        }
        let bad = |error: std::io::Error| FileOpError::invalid_request(format!("bad archive: {error}"));
        let mut archive = tar::Archive::new(Cursor::new(data));
        let (mut entries, mut bytes) = (0u64, 0u64);
        for entry in archive.entries().map_err(bad)? {
            let entry = entry.map_err(bad)?;
            let kind = entry.header().entry_type();
            if !(kind.is_file() || kind.is_dir()) {
                return Err(FileOpError::invalid_request(
                    "archive holds an entry that is neither a file nor a directory (links are not extracted)",
                ));
            }
            archive_relative(&entry.path().map_err(bad)?)?;
            entries += 1;
            bytes = bytes.saturating_add(entry.size());
            if entries > MAX_ARCHIVE_ENTRIES || bytes > FileOp::MAX_ARCHIVE_BYTES {
                return Err(FileOpError::too_large("archive exceeds the entry or size limit"));
            }
        }
        Ok(Self { entries, bytes })
    }

    /// Entries, files and directories, the archive holds.
    pub fn entries(&self) -> u64 {
        self.entries
    }

    /// Total bytes of file content the archive holds.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

/// The `/`-joined relative path of a tar entry, refusing anything that is not a plain descent
/// (absolute, `..`, drive prefixes).
pub(crate) fn archive_relative(path: &Path) -> Result<String, FileOpError> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => {
                return Err(FileOpError::invalid_request(format!(
                    "archive entry {} is not a relative path",
                    path.display()
                )));
            }
        }
    }
    Ok(parts.join("/"))
}
