use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CString, OsString};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use nix::errno::Errno;
use nix::libc;

use super::mount_plan::{FsPlanError, HostNode, HostNodeId, HostSource, HostTree, HostTreeError, NodeKind};

use super::constants::{RESOLVE_NO_MAGICLINKS, RESOLVE_NO_SYMLINKS};

#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

pub struct LinuxHostTree;

fn io_error(path: &Path, source: io::Error) -> HostTreeError {
    HostTreeError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn is_absence(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound
        || matches!(
            error.raw_os_error(),
            Some(libc::ENOTDIR | libc::ELOOP | libc::ENAMETOOLONG)
        )
}

fn is_transient(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::EIO | libc::ESTALE | libc::EMFILE | libc::ENFILE | libc::EAGAIN)
    )
}

impl HostTree for LinuxHostTree {
    fn stat(&self, path: &Path) -> Result<Option<HostNode>, HostTreeError> {
        let metadata = match std::fs::symlink_metadata(path) {
            Err(e) if is_transient(&e) => std::fs::symlink_metadata(path),
            other => other,
        };
        match metadata {
            Ok(meta) => {
                let file_type = meta.file_type();
                let kind = if file_type.is_symlink() {
                    NodeKind::Symlink
                } else if file_type.is_dir() {
                    NodeKind::Dir
                } else if file_type.is_file() {
                    NodeKind::File
                } else {
                    NodeKind::Other
                };
                Ok(Some(HostNode {
                    kind,
                    id: HostNodeId {
                        dev: meta.dev(),
                        ino: meta.ino(),
                    },
                }))
            }
            Err(e) if is_absence(&e) => Ok(None),
            Err(e) => Err(io_error(path, e)),
        }
    }

    fn read_link(&self, path: &Path) -> Result<PathBuf, HostTreeError> {
        std::fs::read_link(path).map_err(|e| io_error(path, e))
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<OsString>, HostTreeError> {
        std::fs::read_dir(path)
            .map_err(|e| io_error(path, e))?
            .map(|entry| entry.map(|e| e.file_name()).map_err(|e| io_error(path, e)))
            .collect()
    }

    fn read_file(&self, path: &Path, max_bytes: usize) -> Result<Option<Vec<u8>>, HostTreeError> {
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) if e.raw_os_error() == Some(libc::ENOTDIR) => return Ok(None),
            Err(e) => return Err(io_error(path, e)),
        };
        if !file.metadata().map_err(|e| io_error(path, e))?.is_file() {
            return Ok(None);
        }
        let mut content = Vec::new();
        file.take(max_bytes as u64)
            .read_to_end(&mut content)
            .map_err(|e| io_error(path, e))?;
        Ok(Some(content))
    }

    fn can_create_in(&self, dir: &Path) -> bool {
        let Some(fd) = open_directory(dir) else {
            return true;
        };
        let held = PathBuf::from(format!("/proc/self/fd/{}", fd.as_raw_fd()));
        match std::fs::read_link(&held) {
            Ok(link) if link == dir => {}
            _ => return true,
        }
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) } != 0 {
            return true;
        }
        let refusal = nix::unistd::access(&held, nix::unistd::AccessFlags::W_OK | nix::unistd::AccessFlags::X_OK).err();
        mount_point_is_makeable(refusal, stat.st_uid == nix::unistd::geteuid().as_raw())
    }
}

fn open_directory(dir: &Path) -> Option<OwnedFd> {
    let path = CString::new(dir.as_os_str().as_bytes()).ok()?;
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_PATH | libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Whether bubblewrap can make a mount point in a directory, from the answer
/// the kernel gives to a write-and-search check. Only a refusal that is not
/// this user's to lift makes it impossible, so that the command cannot create
/// the path either: a mode or ACL (`EACCES`) is overridden by bubblewrap's
/// user namespace for the owner, while a read-only file system (`EROFS`) and
/// an immutable directory (`EPERM`) refuse whoever asks. Any other answer is
/// a doubt, and a doubt plans the mount point.
fn mount_point_is_makeable(refusal: Option<Errno>, owned: bool) -> bool {
    match refusal {
        None => true,
        Some(Errno::EACCES) => owned,
        Some(Errno::EPERM | Errno::EROFS) => false,
        Some(_) => true,
    }
}

/// Opens the planned source without following any symlink and checks that it
/// is still the object the plan was built from, so a path swapped after
/// planning fails the launch instead of being mounted.
pub fn open_source(source: &HostSource) -> Result<OwnedFd, FsPlanError> {
    let open_error = |e: io::Error| FsPlanError::OpenSource {
        path: source.path.clone(),
        source: e,
    };
    let root = unsafe { libc::open(c"/".as_ptr(), libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if root < 0 {
        return Err(open_error(io::Error::last_os_error()));
    }
    let root = unsafe { OwnedFd::from_raw_fd(root) };

    let relative = source
        .path
        .strip_prefix("/")
        .map_err(|_| open_error(io::Error::from(io::ErrorKind::InvalidInput)))?;
    let relative = if relative.as_os_str().is_empty() {
        CString::new(".")
    } else {
        CString::new(relative.as_os_str().as_bytes())
    }
    .map_err(|_| open_error(io::Error::from(io::ErrorKind::InvalidInput)))?;

    let how = OpenHow {
        flags: (libc::O_PATH | libc::O_CLOEXEC) as u64,
        mode: 0,
        resolve: RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS,
    };
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            root.as_raw_fd(),
            relative.as_ptr(),
            &how as *const OpenHow,
            std::mem::size_of::<OpenHow>(),
        )
    };
    if fd < 0 {
        return Err(open_error(io::Error::last_os_error()));
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd as RawFd) };

    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) } != 0 {
        return Err(open_error(io::Error::last_os_error()));
    }
    if stat.st_dev as u64 != source.id.dev || stat.st_ino as u64 != source.id.ino {
        return Err(FsPlanError::SourceChanged {
            path: source.path.clone(),
        });
    }
    Ok(fd)
}

#[derive(Debug, Clone)]
enum MemoryNode {
    Dir,
    File(Vec<u8>),
    Symlink(PathBuf),
    Special,
}

/// A host tree held in memory, for exercising the planner without a
/// filesystem.
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct InMemoryHostTree {
    nodes: BTreeMap<PathBuf, (MemoryNode, u64)>,
    uncreatable: BTreeSet<PathBuf>,
    unlistable: BTreeSet<PathBuf>,
    uninspectable: BTreeSet<PathBuf>,
    next_ino: u64,
}

#[doc(hidden)]
impl InMemoryHostTree {
    pub fn new() -> Self {
        let mut tree = Self::default();
        tree.insert(Path::new("/"), MemoryNode::Dir);
        tree
    }

    fn insert(&mut self, path: &Path, node: MemoryNode) {
        for ancestor in path.ancestors().skip(1) {
            if !self.nodes.contains_key(ancestor) {
                self.next_ino += 1;
                self.nodes
                    .insert(ancestor.to_path_buf(), (MemoryNode::Dir, self.next_ino));
            }
        }
        self.next_ino += 1;
        self.nodes.insert(path.to_path_buf(), (node, self.next_ino));
    }

    pub fn dir(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.insert(path.as_ref(), MemoryNode::Dir);
        self
    }

    pub fn file(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.insert(path.as_ref(), MemoryNode::File(Vec::new()));
        self
    }

    /// A fifo, socket or device node: anything that is not a file, a directory
    /// or a link.
    pub fn special(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.insert(path.as_ref(), MemoryNode::Special);
        self
    }

    pub fn file_with(&mut self, path: &str, content: &str) -> &mut Self {
        self.insert(Path::new(path), MemoryNode::File(content.as_bytes().to_vec()));
        self
    }

    pub fn link(&mut self, path: impl AsRef<Path>, target: impl Into<PathBuf>) -> &mut Self {
        self.insert(path.as_ref(), MemoryNode::Symlink(target.into()));
        self
    }

    pub fn forbid_list(&mut self, dir: &str) -> &mut Self {
        self.unlistable.insert(PathBuf::from(dir));
        self
    }

    pub fn forbid_stat(&mut self, path: &str) -> &mut Self {
        self.uninspectable.insert(PathBuf::from(path));
        self
    }

    pub fn forbid_create(&mut self, dir: &str) -> &mut Self {
        self.uncreatable.insert(PathBuf::from(dir));
        self
    }
}

impl HostTree for InMemoryHostTree {
    fn stat(&self, path: &Path) -> Result<Option<HostNode>, HostTreeError> {
        if self.uninspectable.contains(path) {
            return Err(io_error(path, io::Error::from(io::ErrorKind::PermissionDenied)));
        }
        Ok(self.nodes.get(path).map(|(node, ino)| HostNode {
            kind: match node {
                MemoryNode::Dir => NodeKind::Dir,
                MemoryNode::File(_) => NodeKind::File,
                MemoryNode::Symlink(_) => NodeKind::Symlink,
                MemoryNode::Special => NodeKind::Other,
            },
            id: HostNodeId { dev: 1, ino: *ino },
        }))
    }

    fn read_link(&self, path: &Path) -> Result<PathBuf, HostTreeError> {
        match self.nodes.get(path) {
            Some((MemoryNode::Symlink(target), _)) => Ok(target.clone()),
            _ => Err(io_error(path, io::Error::from(io::ErrorKind::InvalidInput))),
        }
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<OsString>, HostTreeError> {
        if self.unlistable.contains(path) {
            return Err(io_error(path, io::Error::from(io::ErrorKind::PermissionDenied)));
        }
        Ok(self
            .nodes
            .keys()
            .filter(|candidate| candidate.parent() == Some(path) && *candidate != path)
            .filter_map(|candidate| candidate.file_name().map(OsString::from))
            .collect())
    }

    fn read_file(&self, path: &Path, max_bytes: usize) -> Result<Option<Vec<u8>>, HostTreeError> {
        Ok(match self.nodes.get(path) {
            Some((MemoryNode::File(content), _)) => Some(content.iter().copied().take(max_bytes).collect()),
            _ => None,
        })
    }

    fn can_create_in(&self, dir: &Path) -> bool {
        matches!(self.nodes.get(dir), Some((MemoryNode::Dir, _))) && !self.uncreatable.contains(dir)
    }
}

#[cfg(test)]
mod tests;
