//! Claude Code executable discovery in the order of Python `_find_cli` (0.2.181), minus the bundled binary
//! because a Rust crate ships none.

use std::path::{Path, PathBuf};

use crate::{adapters::ReadMetadata, environment_plan::Inherited};

const UNIX_HINT: &str = "Claude Code not found. Install with:\n  npm install -g @anthropic-ai/claude-code\n\nIf already installed locally, try:\n  export PATH=\"$HOME/node_modules/.bin:$PATH\"\n\nOr provide the path via ClaudeAgentOptions:\n  ClaudeAgentOptions(cli_path='/path/to/claude')";
const WINDOWS_HINT: &str = "Claude Code not found. Install the native claude.exe with (PowerShell):\n  irm https://claude.ai/install.ps1 | iex\n\nOr install the claude-agent-sdk wheel for a platform that bundles claude.exe (e.g. Windows x64), or provide the path to a claude.exe via ClaudeAgentOptions:\n  ClaudeAgentOptions(cli_path='C:\\\\path\\\\to\\\\claude.exe')\n\n(npm install -g @anthropic-ai/claude-code produces a claude.cmd shim, which this SDK refuses to run on Windows.)";

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
    use std::os::unix::fs::PermissionsExt;
    filesystem
        .metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable<F: ReadMetadata<Error = std::io::Error>>(filesystem: &F, path: &Path) -> bool {
    is_file(filesystem, path)
}

pub(crate) fn discover<F: ReadMetadata<Error = std::io::Error>>(
    inherited: &Inherited,
    filesystem: &F,
    windows: bool,
) -> Result<PathBuf, CliNotFoundError> {
    let path_variable = inherited.get(std::ffi::OsStr::new("PATH")).cloned().unwrap_or_default();
    let home = inherited
        .get(std::ffi::OsStr::new(if windows { "USERPROFILE" } else { "HOME" }))
        .or_else(|| inherited.get(std::ffi::OsStr::new("HOME")))
        .map(PathBuf::from);
    let mut locations: Vec<PathBuf> = Vec::new();
    if windows {
        for directory in std::env::split_paths(&path_variable) {
            let executable = directory.join("claude.exe");
            if directory.is_absolute() && is_file(filesystem, &executable) {
                return Ok(executable);
            }
        }
        locations.extend(home.iter().map(|home| home.join(".local/bin/claude.exe")));
    } else {
        for directory in std::env::split_paths(&path_variable) {
            let executable = directory.join("claude");
            if !directory.as_os_str().is_empty() && is_executable(filesystem, &executable) {
                return Ok(executable);
            }
        }
        if let Some(home) = &home {
            locations.push(home.join(".npm-global/bin/claude"));
        }
        locations.push(PathBuf::from("/usr/local/bin/claude"));
        if let Some(home) = &home {
            locations.push(home.join(".local/bin/claude"));
            locations.push(home.join("node_modules/.bin/claude"));
            locations.push(home.join(".yarn/bin/claude"));
            locations.push(home.join(".claude/local/claude"));
        }
    }
    locations
        .into_iter()
        .find(|path| is_file(filesystem, path))
        .ok_or(CliNotFoundError {
            message: if windows { WINDOWS_HINT } else { UNIX_HINT },
        })
}

#[cfg(all(test, unix))]
mod tests;
