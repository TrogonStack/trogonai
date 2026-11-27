use std::fs;
use std::io;

use trogon_sandbox_contract::{FileContentHashing, FileOp, FileOpResponse, FileRecursion};
use trogon_sandbox_protocol::file_op_token;

use crate::fs_io::manifest_entry;
use crate::{FileOpError, FsRoot, Manifest};

impl FsRoot {
    /// Serves one operation. Change tracking and watches need state that outlives one call, so
    /// the worker keeps it in a [`Tracking`](crate::Tracking) and reaches this tree through
    /// [`FsRoot::manifest`]; those verbs answer `Unsupported` here.
    pub fn execute(&self, op: &FileOp) -> FileOpResponse {
        self.try_execute(op).unwrap_or_else(FileOpResponse::from)
    }

    /// Like [`FsRoot::execute`], with a failure left as a typed value.
    pub fn try_execute(&self, op: &FileOp) -> Result<FileOpResponse, FileOpError> {
        match op {
            FileOp::List {
                path,
                recursive,
                max_entries,
            } => self.list(path, *recursive, *max_entries),
            FileOp::Stat { path } => self.stat(path),
            FileOp::Read {
                path,
                offset,
                length,
                max_bytes,
            } => self.read(path, *offset, *length, *max_bytes),
            FileOp::Write {
                path,
                data,
                offset,
                truncate,
                mode,
                create_parents,
            } => self.write(path, data, *offset, *truncate, *mode, *create_parents),
            FileOp::Edit {
                path,
                find,
                replace,
                expected_occurrences,
            } => self.edit(path, find, replace, *expected_occurrences),
            FileOp::Search {
                path,
                glob,
                pattern,
                regex,
                case_insensitive,
                max_results,
                max_file_bytes,
            } => self.search(&crate::fs_root_search::SearchRequest {
                path,
                glob: glob.as_deref(),
                pattern: pattern.as_deref(),
                syntax: *regex,
                case: *case_insensitive,
                max_results: *max_results,
                max_file_bytes: *max_file_bytes,
            }),
            FileOp::Remove { path, recursive } => self.remove(path, *recursive),
            FileOp::Mkdir { path, parents } => self.mkdir(path, *parents),
            FileOp::Rename { from, to, overwrite } => self.rename(from, to, *overwrite),
            FileOp::ArchiveGet { path, max_bytes } => self.archive_get(path, *max_bytes),
            FileOp::ArchivePut { path, data } => self.archive_put(path, data),
            FileOp::Checksum { path, offset, length } => self.checksum(path, *offset, *length),
            FileOp::Checkpoint { .. }
            | FileOp::Changes { .. }
            | FileOp::WatchStart { .. }
            | FileOp::WatchPoll { .. }
            | FileOp::WatchStop { .. } => Err(FileOpError::Unsupported {
                message: format!("file operation {} needs worker-side tracking state", file_op_token(op)),
            }),
        }
    }

    /// Fingerprints `path` for change tracking: the one entry for a file, or the entries under a
    /// directory (all depths when recursive, direct children otherwise). Keys are sandbox paths.
    /// Symlinks are recorded, never followed.
    pub fn manifest(
        &self,
        path: &str,
        recursion: FileRecursion,
        hashing: FileContentHashing,
    ) -> Result<Manifest, FileOpError> {
        let hashing = hashing.hashes_content();
        let host = self.resolve(path, false)?;
        let metadata = fs::symlink_metadata(&host).map_err(|e| FileOpError::from_io(e, path))?;
        let base = self.sandbox_path(&self.relative(path)?);
        let mut out = Manifest::new();
        if !metadata.is_dir() {
            out.insert(base, manifest_entry(&host, &metadata, hashing));
            return Ok(out);
        }
        let io = |e: io::Error| FileOpError::from_io(e, path);
        let mut pending = vec![(host, base)];
        while let Some((dir, dir_path)) = pending.pop() {
            for entry in fs::read_dir(&dir).map_err(io)? {
                let entry = entry.map_err(io)?;
                let metadata = entry.metadata().map_err(io)?;
                let name = entry.file_name().to_string_lossy().into_owned();
                let child = if dir_path == "/" {
                    format!("/{name}")
                } else {
                    format!("{dir_path}/{name}")
                };
                if out.len() >= FileOp::TRACKING_MAX_ENTRIES {
                    return Err(FileOpError::too_large(format!(
                        "{path} holds more than {} entries to track",
                        FileOp::TRACKING_MAX_ENTRIES
                    )));
                }
                if recursion.is_recursive() && metadata.is_dir() {
                    pending.push((entry.path(), child.clone()));
                }
                out.insert(child, manifest_entry(&entry.path(), &metadata, hashing));
            }
        }
        Ok(out)
    }
}
