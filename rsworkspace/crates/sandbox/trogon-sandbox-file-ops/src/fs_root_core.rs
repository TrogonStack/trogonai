use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::containment::opened_inside;
use crate::{FileOpError, SandboxPrefix};

pub(crate) type Reply<T> = Result<T, FileOpError>;

/// A directory tree that file operations are confined to.
///
/// [`FsRoot::execute`] runs a [`FileOp`](trogon_sandbox_contract::FileOp) against the host directory and confines every path to it:
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

    /// The sandbox-visible spelling of a root-relative path.
    pub(crate) fn sandbox_path(&self, relative: &Path) -> String {
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
    pub(crate) fn relative(&self, path: &str) -> Reply<PathBuf> {
        if path.contains('\0') {
            return Err(FileOpError::invalid_path("path contains a NUL byte"));
        }
        let inside = match (&self.prefix, path.starts_with('/')) {
            (Some(prefix), true) => path
                .strip_prefix(prefix.as_str())
                .filter(|rest| rest.is_empty() || rest.starts_with('/'))
                .ok_or_else(|| FileOpError::invalid_path(format!("absolute path {path} is outside {prefix}")))?,
            _ => path,
        };
        let mut out = PathBuf::new();
        for component in Path::new(inside).components() {
            match component {
                Component::Normal(part) => out.push(part),
                Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
                Component::ParentDir => {
                    if !out.pop() {
                        return Err(FileOpError::invalid_path(format!(
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
        if opened_inside(file, &self.root).map_err(|e| FileOpError::from_io(e, path))? {
            Ok(())
        } else {
            Err(FileOpError::invalid_path(format!(
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
        let outside = || FileOpError::invalid_path(format!("path {path} resolves outside the sandbox root"));
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
