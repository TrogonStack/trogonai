//! What the upstream path checks ask the host: which platform it is, what a path is on disk, where a link leads, which
//! kind of file system a directory lives on and where the home directory is.
//!
//! The upstream bridge reads those answers from process globals and the real file system. Here they are the methods of
//! [`PlatformProbe`], so the checks can be replayed against scripted answers; [`HostProbe`] is the real host.

use std::ffi::OsStr;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::adapters::{EnvironmentVariable, SystemEnvironment};

/// A platform the upstream checks tell apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// macOS.
    Darwin,
    /// Linux outside the Windows subsystem.
    Linux,
    /// Linux inside the Windows subsystem.
    Wsl,
    /// Windows.
    Win32,
    /// Anything else.
    Other,
}

/// What a path is, as far as the upstream walker cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// A symbolic link.
    Symlink,
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// Something else that exists (a socket, a device, a pipe).
    Special,
    /// Nothing is there, or a parent is not a directory.
    Absent,
    /// The lookup failed for another reason.
    Other,
}

/// Why a probe call failed: the error code when the host gave one, otherwise whether it was a type error.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProbeError {
    /// The error code such as `ENOENT`.
    pub code: Option<String>,
    /// Whether the failure was a type error, which is what a missing call looks like upstream.
    pub type_error: bool,
}

impl ProbeError {
    /// An error with `code`.
    pub fn coded(code: &str) -> Self {
        Self {
            code: Some(code.to_owned()),
            type_error: false,
        }
    }
}

/// The host answers behind the path checks.
pub trait PlatformProbe: Send + Sync {
    /// The platform the process runs on.
    fn platform(&self) -> Platform;
    /// The platform the runtime reports, which tells the Windows subsystem apart from plain Linux.
    fn runtime_platform(&self) -> Platform;
    /// What `path` is, without following a final link.
    fn entry_kind(&self, path: &str) -> EntryKind;
    /// What `path` is once links are followed.
    fn followed_kind(&self, path: &str) -> EntryKind;
    /// The text of the link at `path`, only when it is valid text without a NUL.
    fn link_text(&self, path: &str) -> Option<String>;
    /// The file system type number of `path`.
    fn statfs_type(&self, path: &str) -> Result<i64, ProbeError>;
    /// The home directory, when the host has one.
    fn home_dir(&self) -> Option<String>;
    /// The contents of a text file.
    fn read_text(&self, path: &str) -> Result<String, ProbeError>;
    /// The device a path lives on.
    fn device_of(&self, path: &str) -> Result<u64, ProbeError>;
}

/// The real host.
#[derive(Clone, Copy, Debug, Default)]
pub struct HostProbe;

fn probe_error(error: &io::Error) -> ProbeError {
    #[cfg(unix)]
    let code = error.raw_os_error().map(|errno| match errno {
        libc::ENOENT => "ENOENT".to_owned(),
        libc::ENOTDIR => "ENOTDIR".to_owned(),
        libc::EACCES => "EACCES".to_owned(),
        libc::EPERM => "EPERM".to_owned(),
        libc::ENOSYS => "ENOSYS".to_owned(),
        libc::ENOTSUP => "ENOTSUP".to_owned(),
        errno if errno == libc::EOPNOTSUPP => "EOPNOTSUPP".to_owned(),
        other => format!("E{other}"),
    });
    #[cfg(not(unix))]
    let code = Some(match error.kind() {
        io::ErrorKind::NotFound => "ENOENT".to_owned(),
        io::ErrorKind::PermissionDenied => "EACCES".to_owned(),
        _ => "EIO".to_owned(),
    });
    ProbeError {
        code,
        type_error: false,
    }
}

fn absent(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound || error.kind() == io::ErrorKind::NotADirectory
}

fn kind_of(result: io::Result<std::fs::Metadata>) -> EntryKind {
    match result {
        Ok(meta) => {
            let kind = meta.file_type();
            if kind.is_symlink() {
                EntryKind::Symlink
            } else if kind.is_file() {
                EntryKind::File
            } else if kind.is_dir() {
                EntryKind::Directory
            } else {
                EntryKind::Special
            }
        }
        Err(error) if absent(&error) => EntryKind::Absent,
        Err(_) => EntryKind::Other,
    }
}

impl PlatformProbe for HostProbe {
    fn platform(&self) -> Platform {
        if cfg!(target_os = "macos") {
            Platform::Darwin
        } else if cfg!(target_os = "linux") {
            Platform::Linux
        } else if cfg!(windows) {
            Platform::Win32
        } else {
            Platform::Other
        }
    }

    fn runtime_platform(&self) -> Platform {
        let platform = self.platform();
        if platform != Platform::Linux {
            return platform;
        }
        let subsystem = SystemEnvironment
            .var_os(OsStr::new("WSL_DISTRO_NAME"))
            .is_some_and(|value| !value.is_empty())
            || SystemEnvironment
                .var_os(OsStr::new("WSL_INTEROP"))
                .is_some_and(|value| !value.is_empty())
            || std::fs::read_to_string("/proc/version").is_ok_and(|text| {
                let text = text.to_lowercase();
                text.contains("microsoft") || text.contains("wsl")
            });
        if subsystem { Platform::Wsl } else { Platform::Linux }
    }

    fn entry_kind(&self, path: &str) -> EntryKind {
        kind_of(std::fs::symlink_metadata(path))
    }

    fn followed_kind(&self, path: &str) -> EntryKind {
        kind_of(std::fs::metadata(path))
    }

    fn link_text(&self, path: &str) -> Option<String> {
        std::fs::read_link(path)
            .ok()
            .and_then(|target| target.to_str().map(str::to_owned))
            .filter(|target| !target.contains('\0'))
    }

    #[cfg(unix)]
    fn statfs_type(&self, path: &str) -> Result<i64, ProbeError> {
        let c_path = std::ffi::CString::new(path).map_err(|_| ProbeError::coded("ERR_INVALID_ARG_VALUE"))?;
        // SAFETY: `statfs` writes a plain struct that is fully initialised on success and not read otherwise.
        let mut stats: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: `c_path` is a valid NUL terminated string and `stats` is a valid out pointer.
        let status = unsafe { libc::statfs(c_path.as_ptr(), &mut stats) };
        if status == 0 {
            Ok(widen_fs_type(stats.f_type))
        } else {
            Err(probe_error(&io::Error::last_os_error()))
        }
    }

    #[cfg(not(unix))]
    fn statfs_type(&self, _path: &str) -> Result<i64, ProbeError> {
        Err(ProbeError {
            code: None,
            type_error: true,
        })
    }

    fn home_dir(&self) -> Option<String> {
        let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        SystemEnvironment
            .var_os(OsStr::new(variable))
            .and_then(|home| home.into_string().ok())
            .filter(|home| !home.is_empty())
    }

    fn read_text(&self, path: &str) -> Result<String, ProbeError> {
        std::fs::read(path)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .map_err(|error| probe_error(&error))
    }

    fn device_of(&self, path: &str) -> Result<u64, ProbeError> {
        let meta = std::fs::metadata(Path::new(path)).map_err(|error| probe_error(&error))?;
        #[cfg(unix)]
        {
            Ok(meta.dev())
        }
        #[cfg(not(unix))]
        {
            let _ = meta;
            Ok(0)
        }
    }
}

#[cfg(test)]
#[path = "bridge_platform_tests.rs"]
mod tests;

/// `f_type` is `i64` on Linux and `u32` on macOS; a generic widening keeps one call site for both.
#[cfg(unix)]
fn widen_fs_type<T: Into<i64>>(fs_type: T) -> i64 {
    fs_type.into()
}
