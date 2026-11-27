use std::io::{Error, ErrorKind};

use tokio::process::Command;
use trogon_sandbox_contract::CommandOutput;

/// The result of a command that never started.
///
/// A missing cwd reads the way a shell's failed `cd` does; anything else takes the shell's 127 (not found) or 126 (not
/// runnable), so neither is mistaken for the timeout exit code.
pub(crate) fn spawn_failure(command: &Command, error: &Error) -> CommandOutput {
    let missing_dir = command.as_std().get_current_dir().filter(|dir| !dir.is_dir());
    if let Some(dir) = missing_dir {
        return CommandOutput {
            exit_code: 1,
            stderr: format!("cd: {}: No such file or directory", dir.display()).into_bytes(),
            ..Default::default()
        };
    }
    let exit_code = if error.kind() == ErrorKind::NotFound { 127 } else { 126 };
    CommandOutput {
        exit_code,
        stderr: format!("failed to start command: {error}").into_bytes(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests;
