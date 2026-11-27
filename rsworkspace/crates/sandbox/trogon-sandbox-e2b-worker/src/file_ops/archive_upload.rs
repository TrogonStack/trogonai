use std::collections::BTreeSet;
use std::io::Cursor;

use trogon_sandbox_contract::FileOp;
use trogon_sandbox_file_ops::{ArchiveSummary, FileOpError};

use super::constants::{ARCHIVE_MODE_MASK, TAR_TRAILER_BYTES};
use super::staging::{check_relative, staging_failure};
use super::tar_size::tar_entry_bytes;

/// A vetted tar stream rewritten with canonical ownership and permissions, and the destination paths it creates.
pub(super) struct ArchiveUpload {
    pub(super) data: Vec<u8>,
    pub(super) paths: BTreeSet<String>,
    pub(super) entries: u64,
    pub(super) bytes: u64,
}

impl ArchiveUpload {
    pub(super) fn canonical(data: &[u8]) -> Result<Self, FileOpError> {
        let summary = ArchiveSummary::validate(data)?;
        if summary.entries() > u64::from(FileOp::MAX_ENTRIES) {
            return Err(FileOpError::too_large("archive exceeds the entry limit"));
        }
        let mut archive = tar::Archive::new(Cursor::new(data));
        let mut builder = tar::Builder::new(Vec::new());
        let mut paths = BTreeSet::new();
        let (mut entries, mut bytes) = (0, 0);
        for entry in archive.entries().map_err(staging_failure)? {
            let mut entry = entry.map_err(staging_failure)?;
            let path = entry.path().map_err(staging_failure)?;
            let path = path
                .to_str()
                .ok_or_else(|| FileOpError::invalid_request("archive paths must be UTF-8"))?
                .trim_start_matches("./")
                .trim_end_matches('/')
                .to_string();
            if path.is_empty() || path == "." {
                continue;
            }
            check_relative(&path)?;
            let mut prefix = String::new();
            for component in path.split('/') {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(component);
                paths.insert(prefix.clone());
                if paths.len() > FileOp::MAX_ENTRIES as usize {
                    return Err(FileOpError::too_large("archive has too many destination components"));
                }
            }
            let mut header = entry.header().clone();
            if (builder.get_ref().len() as u64)
                .saturating_add(tar_entry_bytes(&path, entry.size()))
                .saturating_add(TAR_TRAILER_BYTES)
                > FileOp::MAX_ARCHIVE_BYTES
            {
                return Err(FileOpError::too_large("canonical tar stream exceeds the byte limit"));
            }
            header.set_mode(header.mode().map_err(staging_failure)? & ARCHIVE_MODE_MASK);
            header.set_uid(0);
            header.set_gid(0);
            header.set_cksum();
            entries += 1;
            bytes += entry.size();
            builder
                .append_data(&mut header, &path, &mut entry)
                .map_err(staging_failure)?;
        }
        let data = builder.into_inner().map_err(staging_failure)?;
        Ok(Self {
            data,
            paths,
            entries,
            bytes,
        })
    }
}
