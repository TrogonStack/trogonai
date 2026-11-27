//! Directory operations anchored at a descriptor, so a path component swapped for a symlink
//! after the check cannot redirect them. The parent directory is opened once and the kernel
//! is asked where that descriptor landed; the final component is then handled with the
//! `*at` calls, none of which follow a final symlink.

use rustix::fs::{
    AtFlags, CWD, Dir, FileType, Mode, OFlags, RenameFlags, mkdirat, openat, renameat, renameat_with, statat, unlinkat,
};
use rustix::io::Errno;

use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::constants::WRITE_FLAGS;
use crate::containment::require_inside;

fn err(errno: Errno) -> io::Error {
    io::Error::from(errno)
}

fn dir_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC
}

/// Passes over `error` when it is the one `expected`, so a probing call can carry on.
fn tolerate(error: Errno, expected: Errno) -> io::Result<()> {
    if error == expected { Ok(()) } else { Err(err(error)) }
}

/// Opens a directory by path and refuses it when the descriptor landed outside `root`.
fn open_dir(path: &Path, root: &Path) -> io::Result<OwnedFd> {
    let fd = openat(CWD, path, dir_flags(), Mode::empty()).map_err(err)?;
    require_inside(&fd, root)?;
    Ok(fd)
}

/// The verified parent directory of `path` and the final component's name.
fn parent_and_name<'a>(path: &'a Path, root: &Path) -> io::Result<(OwnedFd, &'a OsStr)> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no name"))?;
    let parent = open_dir(path.parent().unwrap_or(root), root)?;
    Ok((parent, name))
}

/// Walks `relative` from the root one component at a time, creating missing directories
/// when `create` is set, and checks every step landed inside the root.
fn walk(root: &Path, relative: &Path, create: bool) -> io::Result<OwnedFd> {
    let mut current = open_dir(root, root)?;
    for part in relative.components() {
        let name = part.as_os_str();
        if create && let Err(e) = mkdirat(&current, name, Mode::from_raw_mode(0o777)) {
            tolerate(e, Errno::EXIST)?;
        }
        let next = openat(&current, name, dir_flags(), Mode::empty()).map_err(err)?;
        require_inside(&next, root)?;
        current = next;
    }
    Ok(current)
}

/// Creates the directory at `host`. Returns whether it already existed.
pub(crate) fn mkdir(root: &Path, host: &Path, parents: bool) -> io::Result<bool> {
    let relative = host.strip_prefix(root).unwrap_or(Path::new(""));
    let Some(name) = relative.file_name() else {
        return Ok(true);
    };
    let parent = walk(root, relative.parent().unwrap_or(Path::new("")), parents)?;
    match statat(&parent, name, AtFlags::empty()) {
        Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::Directory => {
            return Ok(true);
        }
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "exists and is not a directory",
            ));
        }
        Err(_) => {}
    }
    mkdirat(&parent, name, Mode::from_raw_mode(0o777)).map_err(err)?;
    Ok(false)
}

/// Removes `host` without following a final symlink. Returns whether it existed.
pub(crate) fn remove(host: &Path, root: &Path, recursive: bool) -> io::Result<bool> {
    let (parent, name) = match parent_and_name(host, root) {
        Ok(found) => found,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let stat = match statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(Errno::NOENT) => return Ok(false),
        Err(e) => return Err(err(e)),
    };
    if FileType::from_raw_mode(stat.st_mode) == FileType::Directory {
        if recursive {
            remove_tree(&parent, name)?;
        } else {
            unlinkat(&parent, name, AtFlags::REMOVEDIR).map_err(err)?;
        }
    } else {
        unlinkat(&parent, name, AtFlags::empty()).map_err(err)?;
    }
    Ok(true)
}

fn remove_tree(parent: &OwnedFd, name: &OsStr) -> io::Result<()> {
    let dir = openat(parent, name, dir_flags() | OFlags::NOFOLLOW, Mode::empty()).map_err(err)?;
    let mut entries: Vec<(CString, bool)> = Vec::new();
    for entry in Dir::read_from(&dir).map_err(err)? {
        let entry = entry.map_err(err)?;
        let child = entry.file_name();
        if matches!(child.to_bytes(), b"." | b"..") {
            continue;
        }
        let stat = statat(&dir, child, AtFlags::SYMLINK_NOFOLLOW).map_err(err)?;
        let is_dir = FileType::from_raw_mode(stat.st_mode) == FileType::Directory;
        entries.push((child.to_owned(), is_dir));
    }
    for (child, is_dir) in entries {
        if is_dir {
            remove_tree(&dir, OsStr::from_bytes(child.to_bytes()))?;
        } else {
            unlinkat(&dir, child.as_c_str(), AtFlags::empty()).map_err(err)?;
        }
    }
    unlinkat(parent, name, AtFlags::REMOVEDIR).map_err(err)
}

/// Renames between two verified parent directories. The error says whether the source
/// side (`true`) or the target side (`false`) failed.
pub(crate) fn rename(source: &Path, target: &Path, overwrite: bool, root: &Path) -> Result<(), (bool, io::Error)> {
    let (from_dir, from_name) = parent_and_name(source, root).map_err(|e| (true, e))?;
    let (to_dir, to_name) = parent_and_name(target, root).map_err(|e| (false, e))?;
    statat(&from_dir, from_name, AtFlags::SYMLINK_NOFOLLOW).map_err(|e| (true, err(e)))?;
    if overwrite {
        return renameat(&from_dir, from_name, &to_dir, to_name).map_err(|e| (false, err(e)));
    }
    match renameat_with(&from_dir, from_name, &to_dir, to_name, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(e) if lacks_noreplace(e) => rename_if_absent(&from_dir, from_name, &to_dir, to_name),
        Err(e) => Err((false, err(e))),
    }
}

/// Whether the filesystem does not implement the no-replace rename flag.
fn lacks_noreplace(error: Errno) -> bool {
    matches!(error, Errno::INVAL | Errno::NOSYS | Errno::NOTSUP)
}

/// Refuses to replace by looking first, for filesystems without the no-replace rename flag.
pub(crate) fn rename_if_absent(
    from_dir: &OwnedFd,
    from_name: &OsStr,
    to_dir: &OwnedFd,
    to_name: &OsStr,
) -> Result<(), (bool, io::Error)> {
    if statat(to_dir, to_name, AtFlags::SYMLINK_NOFOLLOW).is_ok() {
        return Err((false, io::ErrorKind::AlreadyExists.into()));
    }
    renameat(from_dir, from_name, to_dir, to_name).map_err(|e| (false, err(e)))
}

/// A file opened for writing, plus what it takes to undo its creation.
pub(crate) struct Opened {
    pub(crate) file: File,
    created_in: Option<(OwnedFd, CString)>,
}

impl Opened {
    /// Removes the file when this open created it, so a failed write leaves nothing behind.
    pub(crate) fn discard(self) {
        if let Some((parent, name)) = self.created_in {
            let _ = unlinkat(&parent, name.as_c_str(), AtFlags::empty());
        }
    }
}

/// Opens `path` for writing without following a final symlink, creating it when absent.
pub(crate) fn open_for_write(path: &Path, root: &Path) -> io::Result<Opened> {
    let (parent, name) = parent_and_name(path, root)?;
    match open_existing(&parent, name)? {
        Some(opened) => Ok(opened),
        None => create_new(&parent, name),
    }
}

/// Opens an existing file for writing, or `None` when it is absent.
fn open_existing(parent: &OwnedFd, name: &OsStr) -> io::Result<Option<Opened>> {
    match openat(parent, name, WRITE_FLAGS, Mode::empty()) {
        Ok(fd) => Ok(Some(Opened {
            file: File::from(fd),
            created_in: None,
        })),
        Err(e) => tolerate(e, Errno::NOENT).map(|()| None),
    }
}

/// Creates the file exclusively, so a file another writer created first is never taken over.
pub(crate) fn create_new(parent: &OwnedFd, name: &OsStr) -> io::Result<Opened> {
    let flags = WRITE_FLAGS | OFlags::CREATE | OFlags::EXCL;
    match openat(parent, name, flags, Mode::from_raw_mode(0o666)) {
        Ok(fd) => Ok(Opened {
            file: File::from(fd),
            created_in: Some((parent.try_clone()?, CString::new(name.as_bytes())?)),
        }),
        Err(e) => Err(err(e)),
    }
}
