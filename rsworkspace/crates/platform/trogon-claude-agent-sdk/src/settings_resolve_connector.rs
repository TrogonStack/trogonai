//! Runs the connector folder check against a settings host.
//!
//! The ported check reads paths as the JS strings node hands the file system and reports failures by their node
//! error code, so the host is seen through that lens here: paths cross as UTF-16 code units, and every `io::Error`
//! becomes the `code` node would have put on it.

use std::{
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
};

#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt, OsStringExt};

use crate::{
    adapters::{DirectoryEntryKind, OperatingSystem, SettingsHost},
    connector_folder::names_connector_folder,
    connector_vetting::{Errno, LinkStat, Located, Platform, Vetter, VettingHost},
    constants::VETTING_AUTO_MASTER,
    node_path::{JsString, NodeError, NodeProcess},
    settings_resolve_state::ResolverState,
};

/// A settings host seen through the reads the connector folder check makes.
struct Bridge<'a, H> {
    host: &'a H,
}

/// The text node hands the file system for a JS string: UTF-16 as is on Windows, UTF-8 elsewhere, where a lone
/// surrogate becomes U+FFFD.
#[cfg(windows)]
fn native(text: &[u16]) -> OsString {
    OsString::from_wide(text)
}

#[cfg(not(windows))]
fn native(text: &[u16]) -> OsString {
    OsString::from(String::from_utf16_lossy(text))
}

/// The JS string node makes of a name the file system returned (decoded as UTF-8 outside Windows).
#[cfg(windows)]
fn js_text(text: &OsStr) -> JsString {
    text.encode_wide().collect()
}

#[cfg(not(windows))]
fn js_text(text: &OsStr) -> JsString {
    text.to_string_lossy().encode_utf16().collect()
}

fn native_path(path: &[u16]) -> PathBuf {
    PathBuf::from(native(path))
}

/// The `code` libuv names a raw OS error by, for the codes the check tells apart.
#[cfg(unix)]
fn os_code(code: i32) -> Option<&'static str> {
    Some(match code {
        libc::ENOENT => "ENOENT",
        libc::ENOTDIR => "ENOTDIR",
        libc::EACCES => "EACCES",
        libc::EPERM => "EPERM",
        libc::ENAMETOOLONG => "ENAMETOOLONG",
        libc::ELOOP => "ELOOP",
        libc::EISDIR => "EISDIR",
        libc::EINVAL => "EINVAL",
        libc::EIO => "EIO",
        libc::EBUSY => "EBUSY",
        _ => return None,
    })
}

#[cfg(not(unix))]
fn os_code(_code: i32) -> Option<&'static str> {
    None
}

/// The `code` of the error node would have thrown; an unsupported probe throws a plain `Error` without one.
fn node_code(error: &io::Error) -> Errno {
    if let Some(code) = error.raw_os_error().and_then(os_code) {
        return Some(code.to_owned());
    }
    let code = match error.kind() {
        io::ErrorKind::Unsupported => return None,
        io::ErrorKind::NotFound => "ENOENT",
        io::ErrorKind::PermissionDenied if cfg!(windows) => "EPERM",
        io::ErrorKind::PermissionDenied => "EACCES",
        io::ErrorKind::NotADirectory => "ENOTDIR",
        io::ErrorKind::IsADirectory => "EISDIR",
        io::ErrorKind::InvalidFilename => "ENAMETOOLONG",
        io::ErrorKind::InvalidInput => "EINVAL",
        _ => "UNKNOWN",
    };
    Some(code.to_owned())
}

fn errno(error: io::Error) -> Errno {
    node_code(&error)
}

fn node_error(error: io::Error) -> NodeError {
    NodeError {
        code: node_code(&error),
    }
}

impl<H: SettingsHost> NodeProcess for Bridge<'_, H> {
    fn cwd(&self) -> Result<JsString, NodeError> {
        self.host
            .current_dir()
            .map(|directory| js_text(directory.as_os_str()))
            .map_err(node_error)
    }

    fn env(&self, key: &[u16]) -> Option<JsString> {
        self.host.var_os(&native(key)).map(|value| js_text(&value))
    }
}

impl<H: SettingsHost> VettingHost for Bridge<'_, H> {
    fn lstat(&self, path: &[u16]) -> Result<LinkStat, Errno> {
        let metadata = self.host.symlink_metadata(&native_path(path)).map_err(errno)?;
        Ok(LinkStat {
            symlink: metadata.kind == DirectoryEntryKind::Symlink,
            file: metadata.kind == DirectoryEntryKind::File,
            size: metadata.len,
        })
    }

    fn open_dir_no_follow(&self, path: &[u16]) -> Result<(), Errno> {
        self.host.open_directory_no_follow(&native_path(path)).map_err(errno)
    }

    fn read_link(&self, path: &[u16]) -> Result<JsString, Errno> {
        self.host
            .read_link(&native_path(path))
            .map(|target| js_text(target.as_os_str()))
            .map_err(errno)
    }

    fn read_dir_names(&self, path: &[u16]) -> Result<Vec<JsString>, Errno> {
        let entries = self.host.read_directory(&native_path(path)).map_err(errno)?;
        Ok(entries
            .into_iter()
            .map(|entry| entry.name.encode_utf16().collect())
            .collect())
    }

    fn read_head(&self, path: &[u16], limit: usize) -> Result<Vec<u8>, Errno> {
        self.host.read_file_head(&native_path(path), limit).map_err(errno)
    }

    fn read_auto_master(&self) -> Result<String, Errno> {
        self.host
            .read_file(Path::new(VETTING_AUTO_MASTER))
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .map_err(errno)
    }

    fn stat_device(&self, path: &str) -> Result<u64, Errno> {
        self.host.device_id(Path::new(path)).map_err(errno)
    }

    fn home_directory(&self) -> Result<JsString, Errno> {
        self.host
            .home_directory()
            .map(|home| js_text(home.as_os_str()))
            .map_err(errno)
    }

    fn real_path(&self, path: &[u16]) -> Result<JsString, Errno> {
        self.host
            .canonicalize(&native_path(path))
            .map(|real| js_text(real.as_os_str()))
            .map_err(errno)
    }
}

/// `process.platform`, as far as the check branches on it.
fn platform(system: OperatingSystem) -> Platform {
    match system {
        OperatingSystem::Darwin => Platform::Darwin,
        OperatingSystem::Win32 => Platform::Win32,
        OperatingSystem::Linux | OperatingSystem::Other(_) => Platform::Other,
    }
}

/// Whether the project and local settings of `cwd` must not be read (`VP` and the check in `Hu` upstream): the
/// working directory, resolved the way `path.resolve` does, sits inside a connector download folder that is marked
/// ignored.
pub(crate) fn skips_repository_settings<H: SettingsHost>(host: &H, state: &ResolverState, cwd: &Path) -> bool {
    let bridge = Bridge { host };
    let launch = state
        .launch_directory(host)
        .map(|directory| js_text(directory.as_os_str()));
    let vetter = Vetter::new(
        &bridge,
        platform(host.operating_system()),
        state.node_path_dialect(),
        launch.as_deref(),
        state.vetting(),
    );
    // `path.resolve` reads the process only for a relative path, and the working directory is absolute by now, so
    // resolving it cannot fail; one expression keeps that unreachable failure reading as an unmarked folder.
    let cwd = js_text(cwd.as_os_str());
    vetter.resolve(&[cwd.as_slice()]).is_ok_and(|directory| {
        names_connector_folder(&directory)
            && state
                .connector_folders()
                .is_ignored(&vetter, &directory)
                .unwrap_or(false)
    })
}

/// `Lx(Mx(folder, ...components))` then `Lx(folder)` upstream: where a managed document and the folder it was read
/// from really are, in the order the write check asks.
pub(crate) fn locate_document<H: SettingsHost>(
    host: &H,
    state: &ResolverState,
    folder: &Path,
    components: &[String],
) -> (Located, Located) {
    let bridge = Bridge { host };
    let vetter = Vetter::new(
        &bridge,
        platform(host.operating_system()),
        state.node_path_dialect(),
        None,
        state.vetting(),
    );
    let folder = js_text(folder.as_os_str());
    let components: Vec<JsString> = components
        .iter()
        .map(|component| component.encode_utf16().collect())
        .collect();
    let mut parts: Vec<&[u16]> = vec![folder.as_slice()];
    parts.extend(components.iter().map(Vec::as_slice));
    let document = vetter.locate(&vetter.join(&parts));
    (document, vetter.locate(&folder))
}

#[cfg(all(test, unix))]
#[path = "settings_resolve_connector_tests.rs"]
mod tests;
