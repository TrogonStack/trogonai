//! The home directory, resolved the way each upstream resolves it.
//!
//! TypeScript calls Node's `os.homedir()`: libuv reads `HOME` and returns it even when it is empty, and only when
//! `HOME` is unset asks the passwd database for the effective user. Python calls `Path.home()`, which is
//! `os.path.expanduser("~")`: `HOME` when it is in the environment, even when empty, otherwise the passwd entry of
//! the real user, and a `RuntimeError` when there is none. Both read the process environment, never the child
//! environment passed through the options, so every lookup here goes through the injected
//! [`EnvironmentVariable`] adapter and its passwd methods.

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

mod node_win32;
mod nt_path;

use crate::adapters::EnvironmentVariable;

/// The path rules a lookup follows, so the Windows rules stay testable on any host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Platform {
    Posix,
    Windows,
}

impl Platform {
    pub(crate) const fn host() -> Self {
        if cfg!(windows) { Self::Windows } else { Self::Posix }
    }
}

/// No home directory could be determined: neither the environment nor the passwd database named one.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct HomeDirectoryError {
    message: &'static str,
}

impl HomeDirectoryError {
    /// What `os.homedir()` throws when libuv finds no home.
    const NODE: Self = Self {
        message: "A system error occurred: uv_os_homedir returned ENOENT (no such file or directory)",
    };
    /// What `Path.home()` raises when `expanduser` leaves the `~` in place.
    const PYTHON: Self = Self {
        message: "Could not determine home directory.",
    };

    /// The upstream error text.
    pub fn message(&self) -> &'static str {
        self.message
    }
}

/// Node `os.homedir()`.
///
/// POSIX: `HOME` as set, even when empty, otherwise the effective user's passwd entry. Windows: `USERPROFILE`,
/// where a value shorter than three bytes (including an empty one) is an error, otherwise the passwd lookup.
pub(crate) fn node_homedir<E: EnvironmentVariable>(
    environment: &E,
    platform: Platform,
) -> Result<OsString, HomeDirectoryError> {
    let name = match platform {
        Platform::Posix => "HOME",
        Platform::Windows => "USERPROFILE",
    };
    match environment.var_os(OsStr::new(name)) {
        Some(value) if platform == Platform::Windows && value.as_encoded_bytes().len() < 3 => {
            Err(HomeDirectoryError::NODE)
        }
        Some(value) => Ok(value),
        None => environment
            .effective_user_home_directory()
            .ok_or(HomeDirectoryError::NODE),
    }
}

/// Node `path.join(base, segment)` for one segment without a trailing separator. Node decodes the libuv bytes as
/// lossy UTF-8 before joining, so a base that is not UTF-8 joins as its replacement-character form.
///
/// POSIX paths are normalized the way `path.posix.join` does: an empty base is skipped, `.` and empty parts
/// disappear, `..` removes the part before it (or stays in a relative path) and repeated leading separators
/// collapse. Windows paths go through `path.win32.join`.
pub(crate) fn node_join(platform: Platform, base: &OsStr, segment: &str) -> PathBuf {
    let base = base.to_string_lossy();
    PathBuf::from(match platform {
        Platform::Posix => posix_join(&base, segment),
        Platform::Windows => node_win32::join(&base, segment),
    })
}

fn posix_join(base: &str, segment: &str) -> String {
    let absolute = if base.is_empty() {
        segment.starts_with('/')
    } else {
        base.starts_with('/')
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in base.split('/').chain(segment.split('/')) {
        match part {
            "" | "." => {}
            ".." => match parts.last() {
                Some(&last) if last != ".." => {
                    parts.pop();
                }
                _ if absolute => {}
                _ => parts.push(".."),
            },
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    if absolute { format!("/{joined}") } else { joined }
}

/// Python `Path.home()`.
///
/// The returned path is what `Path` would print, except that an empty path stands for `Path(".")`, which joins as
/// no prefix at all. Join segments with [`python_join`], which is `Path.home() / segment`.
pub(crate) fn python_home<E: EnvironmentVariable>(
    environment: &E,
    platform: Platform,
) -> Result<PathBuf, HomeDirectoryError> {
    match platform {
        Platform::Posix => match posix_expanduser(environment) {
            Some(expanded) if !expanded.as_encoded_bytes().starts_with(b"~") => {
                Ok(PathBuf::from(os_string(pure_posix(expanded.as_encoded_bytes()))))
            }
            _ => Err(HomeDirectoryError::PYTHON),
        },
        Platform::Windows => {
            let variable = |name: &str| {
                environment
                    .var_os(OsStr::new(name))
                    .map(|value| value.to_string_lossy().into_owned())
            };
            match nt_path::expanduser(variable("USERPROFILE"), variable("HOMEDRIVE"), variable("HOMEPATH")) {
                Some(expanded) if !expanded.starts_with('~') => Ok(PathBuf::from(nt_path::normalize(&expanded))),
                _ => Err(HomeDirectoryError::PYTHON),
            }
        }
    }
}

/// Python `Path.home() / segment` for a relative segment, given the path [`python_home`] returned. POSIX joins
/// byte for byte like `posixpath.join` and re-parses like `PurePosixPath`; Windows runs `ntpath.join` and the
/// `PureWindowsPath` parse and format.
pub(crate) fn python_join(platform: Platform, home: &Path, segment: &str) -> PathBuf {
    match platform {
        Platform::Posix => {
            let home = home.as_os_str().as_encoded_bytes();
            let joined = if home.is_empty() || home.ends_with(b"/") {
                [home, segment.as_bytes()].concat()
            } else {
                [home, b"/", segment.as_bytes()].concat()
            };
            PathBuf::from(os_string(pure_posix(&joined)))
        }
        Platform::Windows => PathBuf::from(nt_path::path_join(&home.to_string_lossy(), segment)),
    }
}

/// `posixpath.expanduser("~")`: `HOME` (even empty), else the real user's passwd entry; trailing separators are
/// stripped and an empty result becomes `/`.
fn posix_expanduser<E: EnvironmentVariable>(environment: &E) -> Option<OsString> {
    let home = environment
        .var_os(OsStr::new("HOME"))
        .or_else(|| environment.real_user_home_directory())?;
    let bytes = home.as_encoded_bytes();
    let end = bytes
        .iter()
        .rposition(|&byte| byte != b'/')
        .map_or(0, |index| index + 1);
    Some(if end == 0 {
        OsString::from("/")
    } else {
        os_string(bytes[..end].to_vec())
    })
}

/// What `PurePosixPath` keeps of a path: empty and `.` parts disappear, one or three or more leading separators
/// make one root, exactly two stay two. Byte exact, because Python keeps undecodable bytes as surrogate escapes.
fn pure_posix(path: &[u8]) -> Vec<u8> {
    let mut result: Vec<u8> = if path.starts_with(b"///") || (path.starts_with(b"/") && !path.starts_with(b"//")) {
        b"/".to_vec()
    } else if path.starts_with(b"//") {
        b"//".to_vec()
    } else {
        Vec::new()
    };
    let parts: Vec<&[u8]> = path
        .split(|&byte| byte == b'/')
        .filter(|part| !part.is_empty() && *part != b".")
        .collect();
    result.extend(parts.join(&b'/'));
    result
}

fn os_string(bytes: Vec<u8>) -> OsString {
    // SAFETY: every caller passes bytes cut from `as_encoded_bytes` output at ASCII `/` boundaries, joined or
    // prefixed with ASCII only, which keeps the platform's encoding valid.
    unsafe { OsString::from_encoded_bytes_unchecked(bytes) }
}

#[cfg(test)]
mod tests;
