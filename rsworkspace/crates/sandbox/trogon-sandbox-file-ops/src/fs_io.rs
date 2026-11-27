use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt, chown};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::UNIX_EPOCH;

use sha2::{Digest, Sha256};
use trogon_sandbox_contract::{FileKind, FileSha256, FileStat, FileWriteTruncation};

use crate::containment::require_inside;
use crate::content_hash::encode_digest;
use crate::{ManifestEntry, dirfd};

/// Mutations on the host tree run one at a time, so an edit's read-check-write is never
/// interleaved with another writer in the same worker.
static MUTATION_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn lock() -> MutexGuard<'static, ()> {
    MUTATION_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

// O_NOFOLLOW is applied to the final component: it was canonicalised during resolution, so a link
// swapped in after the check fails the open instead of leading out of the root.
pub(crate) fn open_read(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    nofollow_flag(&mut options);
    options.open(path)
}

#[cfg(unix)]
fn nofollow_flag(options: &mut OpenOptions) {
    options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
}

#[cfg(not(unix))]
fn nofollow_flag(_options: &mut OpenOptions) {}

fn file_name_text(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Swaps new content into `path` with a rename, so a concurrent reader sees either the old or the
/// new file and never a truncated one. The mode of the original is kept.
pub(crate) fn replace_file(path: &Path, content: &[u8], original: &Metadata, root: &Path) -> io::Result<u64> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let temp = dir.join(format!(".{}.edit-{}", file_name_text(path), std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&temp)?;
        require_inside(&file, root)?;
        file.write_all(content)?;
        file.flush()?;
        keep_owner(&temp, original)?;
        fs::set_permissions(&temp, original.permissions())?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        sync_parent(dir);
        Ok(content.len() as u64)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Replaces a whole file through a temp file and a rename, so a reader or a crash mid-write never
/// sees a truncated file. An existing file keeps its mode unless `mode` overrides it. Symlinks and
/// directories take the in-place path, which refuses them the same way.
pub(crate) fn write_whole(path: &Path, data: &[u8], mode: Option<u32>, root: &Path) -> io::Result<u64> {
    let existing = match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return write_at(path, data, 0, FileWriteTruncation::Truncate, mode, root);
        }
        Ok(metadata) => Some(metadata),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    let temp = dir.join(format!(".{}.write-{}", file_name_text(path), std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&temp)?;
        require_inside(&file, root)?;
        file.write_all(data)?;
        file.flush()?;
        if let Some(existing) = &existing {
            keep_owner(&temp, existing)?;
            fs::set_permissions(&temp, existing.permissions())?;
        }
        apply_mode(&temp, mode)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        sync_parent(dir);
        Ok(data.len() as u64)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(unix)]
fn apply_mode(path: &Path, mode: Option<u32>) -> io::Result<()> {
    match mode {
        Some(mode) => fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777)),
        None => Ok(()),
    }
}

#[cfg(not(unix))]
fn apply_mode(_path: &Path, _mode: Option<u32>) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn apply_mode_to_file(file: &File, mode: Option<u32>) -> io::Result<()> {
    match mode {
        Some(mode) => file.set_permissions(fs::Permissions::from_mode(mode & 0o7777)),
        None => Ok(()),
    }
}

#[cfg(not(unix))]
fn apply_mode_to_file(_file: &File, _mode: Option<u32>) -> io::Result<()> {
    Ok(())
}

/// Gives the replacement the owner of the file it replaces, so a rename by a root worker never
/// takes a sandbox user's file away from them. Runs before any mode change because chown can
/// clear setuid and setgid bits. Fails with `PermissionDenied` instead of silently changing the
/// owner when the process cannot chown.
#[cfg(unix)]
fn keep_owner(temp: &Path, original: &Metadata) -> io::Result<()> {
    chown(temp, Some(original.uid()), Some(original.gid()))
}

#[cfg(not(unix))]
fn keep_owner(_temp: &Path, _original: &Metadata) -> io::Result<()> {
    Ok(())
}

/// Best effort on purpose: the rename already happened, and some filesystems refuse to fsync a
/// directory; failing here would report a completed write as lost.
pub(crate) fn sync_parent(dir: &Path) {
    if let Ok(dir) = File::open(dir) {
        let _ = dir.sync_all();
    }
}

pub(crate) fn write_at(
    path: &Path,
    data: &[u8],
    offset: u64,
    truncation: FileWriteTruncation,
    mode: Option<u32>,
    root: &Path,
) -> io::Result<u64> {
    let mut opened = dirfd::open_for_write(path, root)?;
    let result = write_opened(&mut opened.file, data, offset, truncation, mode, root);
    if result.is_err() {
        // A create that did not complete must not leave an empty file behind.
        opened.discard();
    }
    result
}

fn write_opened(
    file: &mut File,
    data: &[u8],
    offset: u64,
    truncation: FileWriteTruncation,
    mode: Option<u32>,
    root: &Path,
) -> io::Result<u64> {
    // Truncate only once the descriptor is known to be inside the root, so a raced path cannot
    // empty a file elsewhere.
    require_inside(file, root)?;
    if truncation.truncates() {
        file.set_len(0)?;
    }
    if offset > 0 {
        file.seek(SeekFrom::Start(offset))?;
    }
    file.write_all(data)?;
    file.flush()?;
    apply_mode_to_file(file, mode)?;
    Ok(file.metadata()?.len())
}

pub(crate) fn stat_of(name: &str, path: &str, metadata: &Metadata, host: &Path) -> FileStat {
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        FileKind::Symlink
    } else if file_type.is_dir() {
        FileKind::Directory
    } else if file_type.is_file() {
        FileKind::File
    } else {
        FileKind::Other
    };
    let modified_unix_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_millis()).ok());
    let symlink_target = file_type
        .is_symlink()
        .then(|| fs::read_link(host).ok())
        .flatten()
        .map(|target| target.to_string_lossy().into_owned());
    FileStat {
        path: path.to_string(),
        name: name.to_string(),
        kind,
        size: if kind == FileKind::File { metadata.len() } else { 0 },
        mode: mode_bits(metadata),
        modified_unix_ms,
        symlink_target,
    }
}

#[cfg(unix)]
fn mode_bits(metadata: &Metadata) -> Option<u32> {
    Some(metadata.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn mode_bits(_metadata: &Metadata) -> Option<u32> {
    None
}

pub(crate) fn sorted_entries(dir: &Path) -> io::Result<Vec<fs::DirEntry>> {
    let mut entries = fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}

pub(crate) fn manifest_entry(host: &Path, metadata: &Metadata, hashing: bool) -> ManifestEntry {
    if metadata.is_dir() {
        return ManifestEntry::directory();
    }
    let stamp = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| format!("{}.{:09}", duration.as_secs(), duration.subsec_nanos()))
        .unwrap_or_default();
    let hash = (hashing && metadata.is_file()).then(|| hash_file(host).ok()).flatten();
    ManifestEntry::file(metadata.len(), Some(stamp), hash)
}

fn hash_file(path: &Path) -> io::Result<FileSha256> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    encode_digest(&hasher.finalize()).map_err(io::Error::other)
}
