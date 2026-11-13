use std::ffi::OsStr;

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
        python_repr(&.path.to_string_lossy())
    )]
    WindowsBatchScript { path: std::path::PathBuf },
}

#[cfg(windows)]
pub(crate) fn reject_windows_batch_cli(path: &OsStr, policy: WindowsBatchCli) -> Result<(), CliPathError> {
    if policy == WindowsBatchCli::Refuse && is_windows_batch_cli(path) {
        return Err(CliPathError::WindowsBatchScript { path: path.into() });
    }
    Ok(())
}

#[cfg(not(windows))]
pub(crate) fn reject_windows_batch_cli(_: &OsStr, _: WindowsBatchCli) -> Result<(), CliPathError> {
    Ok(())
}

#[cfg(any(windows, test))]
fn is_windows_batch_cli(path: &OsStr) -> bool {
    path.to_string_lossy().replace('\\', "/").split('/').any(|part| {
        part.split(':').any(|segment| {
            let segment = segment.trim_end_matches(['.', ' ']).to_ascii_lowercase();
            segment.ends_with(".bat") || segment.ends_with(".cmd")
        })
    })
}

/// Python `repr` of a `str`, which the Python SDK uses to quote the refused path.
fn python_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut output = String::with_capacity(text.len() + 2);
    output.push(quote);
    for character in text.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character == quote => {
                output.push('\\');
                output.push(character);
            }
            character if (character as u32) < 0x20 || character as u32 == 0x7f => {
                output.push_str(&format!("\\x{:02x}", character as u32));
            }
            character => output.push(character),
        }
    }
    output.push(quote);
    output
}

#[cfg(test)]
mod tests;
