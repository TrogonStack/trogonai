//! Claude Code executable discovery in the order of Python `_find_cli` (0.2.181), minus the bundled binary
//! because a Rust crate ships none.

use crate::constants::{UNIX_HINT, WINDOWS_HINT};
use std::path::{Path, PathBuf};

use crate::{
    Error,
    adapters::{EnvironmentVariable, ReadMetadata},
    environment_plan::Inherited,
    home_directory::{HomeDirectoryError, Platform, python_home, python_join},
};

/// Why discovery failed: nothing was found, or Python's `Path.home()` raised while building the location list.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum CliDiscoveryError {
    #[error(transparent)]
    NotFound(#[from] CliNotFoundError),
    #[error(transparent)]
    Home(#[from] HomeDirectoryError),
}

impl From<CliDiscoveryError> for Error {
    fn from(error: CliDiscoveryError) -> Self {
        match error {
            CliDiscoveryError::NotFound(error) => Self::CliNotFound(error),
            CliDiscoveryError::Home(error) => Self::HomeDirectory(error),
        }
    }
}

/// No Claude Code executable was found on `PATH` or in the well-known install locations.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct CliNotFoundError {
    message: &'static str,
}

impl CliNotFoundError {
    pub fn message(&self) -> &'static str {
        self.message
    }
}

fn is_file<F: ReadMetadata<Error = std::io::Error>>(filesystem: &F, path: &Path) -> bool {
    filesystem.metadata(path).is_ok_and(|metadata| metadata.is_file())
}

#[cfg(unix)]
fn is_executable<F: ReadMetadata<Error = std::io::Error>>(filesystem: &F, path: &Path) -> bool {
    filesystem.metadata(path).is_ok_and(|metadata| {
        metadata.is_file() && std::os::unix::fs::PermissionsExt::mode(&metadata.permissions()) & 0o111 != 0
    })
}

#[cfg(not(unix))]
fn is_executable<F: ReadMetadata<Error = std::io::Error>>(filesystem: &F, path: &Path) -> bool {
    is_file(filesystem, path)
}

/// Python `_find_cli`: the `PATH` lookup first, then the well-known install locations under `Path.home()`. The
/// home is read from the process environment like `Path.home()` does and only after the `PATH` lookup missed,
/// because the location list is built after `shutil.which`; a home that cannot be determined raises there.
pub(crate) fn discover<E: EnvironmentVariable, F: ReadMetadata<Error = std::io::Error>>(
    environment: &E,
    inherited: &Inherited,
    filesystem: &F,
    windows: bool,
) -> Result<PathBuf, CliDiscoveryError> {
    let path_variable = inherited.get(std::ffi::OsStr::new("PATH")).cloned().unwrap_or_default();
    let platform = if windows { Platform::Windows } else { Platform::Posix };
    let locations: Vec<PathBuf> = if windows {
        for directory in std::env::split_paths(&path_variable) {
            let executable = directory.join("claude.exe");
            if directory.is_absolute() && is_file(filesystem, &executable) {
                return Ok(executable);
            }
        }
        vec![python_join(
            platform,
            &python_home(environment, platform)?,
            ".local/bin/claude.exe",
        )]
    } else {
        for directory in std::env::split_paths(&path_variable) {
            let executable = directory.join("claude");
            if !directory.as_os_str().is_empty() && is_executable(filesystem, &executable) {
                return Ok(executable);
            }
        }
        let home = python_home(environment, platform)?;
        let under_home = |segment: &str| python_join(platform, &home, segment);
        vec![
            under_home(".npm-global/bin/claude"),
            PathBuf::from("/usr/local/bin/claude"),
            under_home(".local/bin/claude"),
            under_home("node_modules/.bin/claude"),
            under_home(".yarn/bin/claude"),
            under_home(".claude/local/claude"),
        ]
    };
    locations
        .into_iter()
        .find(|path| is_file(filesystem, path))
        .ok_or(CliDiscoveryError::NotFound(CliNotFoundError {
            message: if windows { WINDOWS_HINT } else { UNIX_HINT },
        }))
}

#[cfg(all(test, unix))]
mod tests;
