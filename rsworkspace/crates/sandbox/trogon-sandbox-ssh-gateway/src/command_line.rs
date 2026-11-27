use std::collections::BTreeMap;

use trogon_sandbox_contract::TerminalArgument;

use crate::command_line_error::CommandLineError;

/// Single-quotes `text` for a POSIX shell.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn valid_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(byte) if byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// The command line to run in the PTY, or `None` for a plain login shell. `cwd` and `env` are applied by the shell that
/// runs the line, since the sandbox's `sshd` may refuse `env` requests.
pub(crate) fn build_command(
    cmd: &[TerminalArgument],
    cwd: Option<&str>,
    env: &BTreeMap<String, String>,
) -> Result<Option<String>, CommandLineError> {
    if cmd.is_empty() && cwd.is_none() && env.is_empty() {
        return Ok(None);
    }
    let mut line = String::new();
    if let Some(cwd) = cwd {
        line.push_str(&format!("cd {} || exit 1; ", quote(cwd)));
    }
    for (name, value) in env {
        if !valid_env_name(name) {
            return Err(CommandLineError::InvalidEnvName(name.clone()));
        }
        line.push_str(&format!("export {name}={}; ", quote(value)));
    }
    if cmd.is_empty() {
        line.push_str("exec \"${SHELL:-/bin/sh}\"");
    } else {
        let argv: Vec<String> = cmd.iter().map(|argument| quote(argument.as_str())).collect();
        line.push_str(&format!("exec {}", argv.join(" ")));
    }
    Ok(Some(line))
}
