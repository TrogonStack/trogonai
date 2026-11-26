use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use trogon_sandbox_contract::{FileContentHashing, FileOp, FileOpResponse, FileRecursion};
use trogon_sandbox_protocol::file_op_token;

use crate::fs_io::{manifest_entry, opened_inside};
use crate::{FileOpFailure, Manifest, SandboxPrefix};

pub(crate) type Reply<T> = Result<T, FileOpFailure>;

/// A directory tree that file operations are confined to.
///
/// [`FsRoot::execute`] runs a [`FileOp`] against the host directory and confines every path to it:
/// `..` cannot climb above the root and a symlink that resolves outside the root is refused. The
/// symlink check runs before the operation, so a process in the sandbox could swap a component for
/// a link in between. Operations that open a file (read, checksum, edit, write, and the temp file
/// behind a whole-file replace) therefore also ask the kernel where the open descriptor landed
/// (`/proc/self/fd` on Linux, `F_GETPATH` on macOS) and refuse it before changing anything when it
/// sits outside the root; a create whose check fails removes the file it made. Directory
/// operations (mkdir, remove, rename) open the parent directory, check where that descriptor
/// landed, and act on the final component relative to it (`mkdirat`, `unlinkat`, `renameat`), so a
/// swap after the path check cannot redirect them.
///
/// A worker whose sandbox filesystem is visible on the host (a workspace directory, or a tree
/// staged out of a container) serves the whole `file_op` verb by calling [`FsRoot::execute`].
#[derive(Debug, Clone)]
pub struct FsRoot {
    pub(crate) root: PathBuf,
    prefix: Option<SandboxPrefix>,
}

impl FsRoot {
    /// `sandbox_prefix` is where the root appears inside the sandbox (for example `/workspace`).
    /// With a prefix, an absolute path must lie under it and a relative path resolves against it.
    /// Without one, an absolute path is read from the root itself.
    pub fn new(root: impl AsRef<Path>, sandbox_prefix: Option<SandboxPrefix>) -> io::Result<Self> {
        Ok(Self {
            root: root.as_ref().canonicalize()?,
            prefix: sandbox_prefix,
        })
    }

    /// Serves one operation. Change tracking and watches need state that outlives one call, so
    /// the worker keeps it in a [`Tracking`](crate::Tracking) and reaches this tree through
    /// [`FsRoot::manifest`]; those verbs answer `Unsupported` here.
    pub fn execute(&self, op: &FileOp) -> FileOpResponse {
        self.try_execute(op).unwrap_or_else(FileOpResponse::from)
    }

    /// Like [`FsRoot::execute`], with a failure left as a typed value.
    pub fn try_execute(&self, op: &FileOp) -> Result<FileOpResponse, FileOpFailure> {
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
            | FileOp::WatchStop { .. } => Err(FileOpFailure::Unsupported {
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
    ) -> Result<Manifest, FileOpFailure> {
        let hashing = hashing.hashes_content();
        let host = self.resolve(path, false)?;
        let metadata = fs::symlink_metadata(&host).map_err(|e| FileOpFailure::from_io(e, path))?;
        let base = self.sandbox_path(&self.relative(path)?);
        let mut out = Manifest::new();
        if !metadata.is_dir() {
            out.insert(base, manifest_entry(&host, &metadata, hashing));
            return Ok(out);
        }
        let io = |e: io::Error| FileOpFailure::from_io(e, path);
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
                    return Err(FileOpFailure::too_large(format!(
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

    /// The sandbox-visible spelling of a root-relative path.
    fn sandbox_path(&self, relative: &Path) -> String {
        let rel = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let prefix = self.prefix.as_ref().map_or("", SandboxPrefix::as_str);
        match (prefix.is_empty(), rel.is_empty()) {
            (true, true) => "/".to_string(),
            (true, false) => format!("/{rel}"),
            (false, true) => prefix.to_string(),
            (false, false) => format!("{prefix}/{rel}"),
        }
    }

    /// The path relative to the root that `path` names, with `.` removed and `..` folded.
    fn relative(&self, path: &str) -> Reply<PathBuf> {
        if path.contains('\0') {
            return Err(FileOpFailure::invalid_path("path contains a NUL byte"));
        }
        let inside = match (&self.prefix, path.starts_with('/')) {
            (Some(prefix), true) => path
                .strip_prefix(prefix.as_str())
                .filter(|rest| rest.is_empty() || rest.starts_with('/'))
                .ok_or_else(|| FileOpFailure::invalid_path(format!("absolute path {path} is outside {prefix}")))?,
            _ => path,
        };
        let mut out = PathBuf::new();
        for component in Path::new(inside).components() {
            match component {
                Component::Normal(part) => out.push(part),
                Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
                Component::ParentDir => {
                    if !out.pop() {
                        return Err(FileOpFailure::invalid_path(format!(
                            "path {path} climbs above the sandbox root"
                        )));
                    }
                }
            }
        }
        Ok(out)
    }

    /// `resolve` checks the path before it is opened, so a process inside the sandbox that swaps a
    /// directory for a symlink in between could steer the open outside the root. Checking where
    /// the opened descriptor really points closes that window for every operation that reads or
    /// writes through a descriptor.
    pub(crate) fn ensure_opened_inside(&self, file: &fs::File, path: &str) -> Reply<()> {
        if opened_inside(file, &self.root).map_err(|e| FileOpFailure::from_io(e, path))? {
            Ok(())
        } else {
            Err(FileOpFailure::invalid_path(format!(
                "path {path} resolves outside the sandbox root"
            )))
        }
    }

    /// Resolves `path` to a host path inside the root. With `follow_final`, a final symlink is
    /// followed and its target must also lie inside the root; without it the link itself is the
    /// subject (stat, remove, rename).
    pub(crate) fn resolve(&self, path: &str, follow_final: bool) -> Reply<PathBuf> {
        let relative = self.relative(path)?;
        let full = self.root.join(&relative);
        if relative.as_os_str().is_empty() {
            return Ok(full);
        }
        let outside = || FileOpFailure::invalid_path(format!("path {path} resolves outside the sandbox root"));
        let mut ancestor = full.parent();
        while let Some(dir) = ancestor {
            match dir.canonicalize() {
                Ok(canonical) => {
                    if !canonical.starts_with(&self.root) {
                        return Err(outside());
                    }
                    break;
                }
                Err(_) => ancestor = dir.parent(),
            }
        }
        if follow_final && let Ok(canonical) = full.canonicalize() {
            if !canonical.starts_with(&self.root) {
                return Err(outside());
            }
            return Ok(canonical);
        }
        Ok(full)
    }
}
