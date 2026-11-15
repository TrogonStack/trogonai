use std::{ffi::OsStr, path::Path};

/// Whether a Windows `.bat` or `.cmd` CLI path may be started.
///
/// The TypeScript SDK spawns whatever path it resolves, so `Run` is the default. `Refuse` selects the
/// Python SDK, which refuses batch scripts on Windows before spawning because `cmd.exe` re-parses
/// their arguments and has no reliable escaping.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WindowsBatchCli {
    #[default]
    Run,
    Refuse,
}

/// Refusal to start a CLI path that Windows would interpret through a shell.
#[derive(Debug, thiserror::Error)]
pub enum CliPathError {
    #[error(
        "Refusing to execute batch script {}: Windows runs .bat/.cmd files via cmd.exe, which can execute commands injected through CLI arguments, and no reliable escaping for cmd.exe exists. Use a native claude executable instead: install Claude Code natively (irm https://claude.ai/install.ps1 | iex), point ClaudeAgentOptions(cli_path=...) at a claude.exe, or install the claude-agent-sdk wheel for a platform that bundles claude.exe (e.g. Windows x64).",
        quoted(.path)
    )]
    WindowsBatchScript { path: std::path::PathBuf },
}

pub(crate) fn reject_windows_batch_cli(path: &OsStr, policy: WindowsBatchCli) -> Result<(), CliPathError> {
    refuse_batch_cli(path, policy, cfg!(windows))
}

/// Python asks `platform.system() == "Windows"` at run time; `windows` is that answer.
fn refuse_batch_cli(path: &OsStr, policy: WindowsBatchCli, windows: bool) -> Result<(), CliPathError> {
    if policy == WindowsBatchCli::Refuse && windows && is_windows_batch_cli(path) {
        return Err(CliPathError::WindowsBatchScript { path: path.into() });
    }
    Ok(())
}

/// Python's `any(segment.rstrip(". ").lower().endswith((".bat", ".cmd")) ...)` over every `/`, `\` and `:` part.
fn is_windows_batch_cli(path: &OsStr) -> bool {
    path.to_string_lossy().replace('\\', "/").split('/').any(|part| {
        part.split(':').any(|segment| {
            let segment = segment.trim_end_matches(['.', ' ']).to_lowercase();
            segment.ends_with(".bat") || segment.ends_with(".cmd")
        })
    })
}

/// Python `repr` of the `str` it holds for the path.
fn quoted(path: &Path) -> String {
    crate::python_text::repr(&crate::python_text::from_os_str(path.as_os_str()))
}

#[cfg(test)]
mod tests;
