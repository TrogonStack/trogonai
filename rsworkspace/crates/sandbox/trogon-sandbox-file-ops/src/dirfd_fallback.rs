//! Directory operations anchored at a descriptor, so a path component swapped for a symlink
//! after the check cannot redirect them. The parent directory is opened once and the kernel
//! is asked where that descriptor landed; the final component is then handled with the
//! `*at` calls, none of which follow a final symlink.

use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::Path;

pub(crate) fn mkdir(_root: &Path, host: &Path, parents: bool) -> io::Result<bool> {
    if let Ok(metadata) = fs::metadata(host) {
        return if metadata.is_dir() {
            Ok(true)
        } else {
            Err(io::ErrorKind::AlreadyExists.into())
        };
    }
    if parents {
        fs::create_dir_all(host)?;
    } else {
        fs::create_dir(host)?;
    }
    Ok(false)
}

pub(crate) fn remove(host: &Path, _root: &Path, recursive: bool) -> io::Result<bool> {
    let metadata = match fs::symlink_metadata(host) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    if metadata.is_dir() {
        if recursive {
            fs::remove_dir_all(host)?;
        } else {
            fs::remove_dir(host)?;
        }
    } else {
        fs::remove_file(host)?;
    }
    Ok(true)
}

pub(crate) fn rename(source: &Path, target: &Path, overwrite: bool, _root: &Path) -> Result<(), (bool, io::Error)> {
    fs::symlink_metadata(source).map_err(|e| (true, e))?;
    if !overwrite && fs::symlink_metadata(target).is_ok() {
        return Err((false, io::ErrorKind::AlreadyExists.into()));
    }
    fs::rename(source, target).map_err(|e| (false, e))
}

pub(crate) struct Opened {
    pub(crate) file: File,
    created: Option<std::path::PathBuf>,
}

impl Opened {
    pub(crate) fn discard(self) {
        if let Some(path) = self.created {
            let _ = fs::remove_file(path);
        }
    }
}

pub(crate) fn open_for_write(path: &Path, _root: &Path) -> io::Result<Opened> {
    let existed = path.exists();
    let file = OpenOptions::new().write(true).create(true).truncate(false).open(path)?;
    Ok(Opened {
        file,
        created: (!existed).then(|| path.to_path_buf()),
    })
}
