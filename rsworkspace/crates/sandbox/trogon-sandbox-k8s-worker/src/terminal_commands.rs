use std::collections::BTreeMap;

use trogon_sandbox_contract::TerminalId;

use crate::constants::DEFAULT_TERM;
use crate::shell_wrap::shell_wrap;

/// Where the wrapper records the command's pid inside the pod.
pub(crate) fn pid_file(id: &TerminalId) -> String {
    format!("/tmp/.sbx-term-{}", id.as_str())
}

/// The argv that starts `cmd` (or a shell) with `cwd` and `env` applied and
/// records its pid first. `exec` keeps the pid, so signalling it signals the
/// command itself.
pub(crate) fn wrapped_command(
    id: &TerminalId,
    cmd: &[String],
    cwd: Option<&str>,
    env: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut inner = vec![
        "sh".to_string(),
        "-c".to_string(),
        format!("set -e; echo $$ > {}; exec \"$@\"", pid_file(id)),
        "sh".to_string(),
    ];
    if cmd.is_empty() {
        inner.push("/bin/sh".to_string());
    } else {
        inner.extend(cmd.iter().cloned());
    }
    let mut env = env.clone();
    env.entry("TERM".to_string())
        .or_insert_with(|| DEFAULT_TERM.to_string());
    shell_wrap(&inner, cwd, &env, None)
}

#[cfg(test)]
mod tests;
