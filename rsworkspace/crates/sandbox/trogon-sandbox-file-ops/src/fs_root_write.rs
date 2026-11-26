use std::fs;
use std::io::{self, Read};

use trogon_sandbox_contract::{
    Base64Data, FileOp, FileOpErrorKind, FileOpResponse, FileOverwrite, FileParentCreation, FilePresence,
    FileRecursion, FileWriteTruncation,
};

use crate::fs_io::{lock, open_read, replace_file, write_at, write_whole};
use crate::fs_root::Reply;
use crate::fs_root_read::directory_failure;
use crate::{EditOutcome, EditSpec, FileOpFailure, FsRoot, dirfd, payload};

impl FsRoot {
    pub(crate) fn write(
        &self,
        path: &str,
        data: &Base64Data,
        offset: u64,
        truncation: FileWriteTruncation,
        mode: Option<u32>,
        parents: FileParentCreation,
    ) -> Reply<FileOpResponse> {
        let io = |e| FileOpFailure::from_io(e, path);
        let size_of_request = payload::decoded_len(data);
        if size_of_request > FileOp::MAX_WRITE_BYTES {
            return Err(FileOpFailure::too_large(format!(
                "write of {size_of_request} bytes exceeds the {} byte per-request limit; send the file in chunks",
                FileOp::MAX_WRITE_BYTES
            )));
        }
        let bytes = payload::decode(data);
        let host = self.resolve(path, true)?;
        let _guard = lock();
        if host == self.root {
            return Err(FileOpFailure::failed(
                FileOpErrorKind::IsADirectory,
                "cannot write the root",
            ));
        }
        if parents.creates()
            && let Some(parent) = host.parent()
        {
            fs::create_dir_all(parent).map_err(io)?;
        }
        let size = if offset == 0 && truncation.truncates() {
            write_whole(&host, &bytes, mode, &self.root)
        } else {
            write_at(&host, &bytes, offset, truncation, mode, &self.root)
        }
        .map_err(io)?;
        Ok(FileOpResponse::Written {
            bytes: bytes.len() as u64,
            size,
        })
    }

    pub(crate) fn edit(&self, path: &str, find: &str, replace: &str, expected: u32) -> Reply<FileOpResponse> {
        let io = |e| FileOpFailure::from_io(e, path);
        let spec = EditSpec::new(find, replace, expected)?;
        let host = self.resolve(path, true)?;
        let _guard = lock();
        let mut file = open_read(&host).map_err(io)?;
        self.ensure_opened_inside(&file, path)?;
        let metadata = file.metadata().map_err(io)?;
        if metadata.is_dir() {
            return Err(directory_failure(path));
        }
        if metadata.len() > FileOp::MAX_READ_BYTES {
            return Err(FileOpFailure::too_large(format!(
                "{path} is {} bytes, over the {} byte edit limit",
                metadata.len(),
                FileOp::MAX_READ_BYTES
            )));
        }
        let mut content = Vec::with_capacity(metadata.len() as usize);
        file.read_to_end(&mut content).map_err(io)?;
        drop(file);
        match spec.apply(&content)? {
            EditOutcome::Rejected { occurrences } => Ok(FileOpResponse::EditRejected { occurrences, expected }),
            EditOutcome::Replaced { content, occurrences } => {
                let size = replace_file(&host, &content, &metadata, &self.root).map_err(io)?;
                Ok(FileOpResponse::Edited { occurrences, size })
            }
        }
    }

    pub(crate) fn remove(&self, path: &str, recursion: FileRecursion) -> Reply<FileOpResponse> {
        let host = self.resolve(path, false)?;
        if host == self.root {
            return Err(FileOpFailure::invalid_path("cannot remove the sandbox root"));
        }
        let _guard = lock();
        let existed =
            dirfd::remove(&host, &self.root, recursion.is_recursive()).map_err(|e| FileOpFailure::from_io(e, path))?;
        Ok(FileOpResponse::Removed {
            existed: FilePresence::from(existed),
        })
    }

    pub(crate) fn mkdir(&self, path: &str, parents: FileParentCreation) -> Reply<FileOpResponse> {
        let host = self.resolve(path, true)?;
        let _guard = lock();
        let already_existed = dirfd::mkdir(&self.root, &host, parents.creates()).map_err(|e| match e.kind() {
            io::ErrorKind::AlreadyExists => FileOpFailure::failed(
                FileOpErrorKind::AlreadyExists,
                format!("{path} exists and is not a directory"),
            ),
            _ => FileOpFailure::from_io(e, path),
        })?;
        Ok(FileOpResponse::Created {
            already_existed: FilePresence::from(already_existed),
        })
    }

    pub(crate) fn rename(&self, from: &str, to: &str, overwrite: FileOverwrite) -> Reply<FileOpResponse> {
        let source = self.resolve(from, false)?;
        let target = self.resolve(to, false)?;
        if source == self.root || target == self.root {
            return Err(FileOpFailure::invalid_path("cannot rename the sandbox root"));
        }
        let _guard = lock();
        let overwrites = overwrite.overwrites();
        dirfd::rename(&source, &target, overwrites, &self.root).map_err(|(is_source, e)| {
            if !is_source && !overwrites && e.kind() == io::ErrorKind::AlreadyExists {
                FileOpFailure::failed(FileOpErrorKind::AlreadyExists, format!("{to} already exists"))
            } else {
                FileOpFailure::from_io(e, if is_source { from } else { to })
            }
        })?;
        Ok(FileOpResponse::Renamed)
    }
}
