//! Names the guest tools a hosted lane needs and reports which of them a sandbox image lacks.

use trogon_sandbox_contract::{CommandOutput, RunCommandResponse};

/// Largest stdout the diagnosis reads.
pub const MAX_DIAGNOSTIC_BYTES: usize = 2048;

/// The exit code the required-tools probe uses when a tool is missing.
pub const MISSING_TOOL_EXIT: i32 = 42;

/// Exits `MISSING_TOOL_EXIT` unless every tool the lanes use is present, with `nc` or a Python 3 socket as the probe.
pub const REQUIRED_TOOLS_COMMAND: &str = "for t in sh timeout mkfifo base64 node stty sleep cat; do \
command -v \"$t\" >/dev/null 2>&1 || exit 42; done; \
if command -v nc >/dev/null 2>&1; then exit 0; fi; \
if command -v python3 >/dev/null 2>&1 && python3 -c 'import socket' >/dev/null 2>&1; then exit 0; fi; \
exit 42";

/// Prints one `snapshot_tool_<name>_present` or `_absent` line per tool, then the state of the Python 3 socket module.
pub const COMMAND: &str = "for t in sh timeout mkfifo base64 node stty nc sleep cat python3; do \
if command -v \"$t\" >/dev/null 2>&1; then echo \"snapshot_tool_${t}_present\"; \
else echo \"snapshot_tool_${t}_absent\"; fi; done; \
if command -v python3 >/dev/null 2>&1 && python3 -c 'import socket' >/dev/null 2>&1; \
then echo snapshot_python3_socket_usable; else echo snapshot_python3_socket_unavailable; fi";

/// Reported when the diagnosis command itself gave no usable answer.
pub const UNAVAILABLE: &str = "snapshot_prerequisite_diagnosis_unavailable";

const TOOLS: &[&str] = &[
    "sh", "timeout", "mkfifo", "base64", "node", "stty", "nc", "sleep", "cat", "python3",
];

const PYTHON_STATES: &[&str] = &["snapshot_python3_socket_usable", "snapshot_python3_socket_unavailable"];

/// Whether the required-tools probe says the image is missing something.
pub fn needs_diagnosis(response: &RunCommandResponse) -> bool {
    matches!(response, RunCommandResponse::Completed(output) if output.exit_code == MISSING_TOOL_EXIT)
}

fn known_checks() -> Vec<String> {
    let mut checks = Vec::new();
    for tool in TOOLS {
        checks.push(format!("snapshot_tool_{tool}_present"));
        checks.push(format!("snapshot_tool_{tool}_absent"));
    }
    checks.extend(PYTHON_STATES.iter().map(|state| (*state).to_owned()));
    checks
}

fn tokens(output: &CommandOutput) -> Vec<String> {
    let bounded = &output.stdout[..output.stdout.len().min(MAX_DIAGNOSTIC_BYTES)];
    let known = known_checks();
    String::from_utf8_lossy(bounded)
        .lines()
        .map(str::trim)
        .filter(|line| known.iter().any(|known| known == line))
        .map(str::to_owned)
        .collect()
}

/// The fixed tokens a diagnosis command printed. Only known tokens are returned, so image output cannot reach a
/// report as free text.
pub fn diagnosis_checks(response: Option<&RunCommandResponse>) -> Vec<String> {
    let Some(RunCommandResponse::Completed(output)) = response else {
        return vec![UNAVAILABLE.to_owned()];
    };
    let found = tokens(output);
    if found.is_empty() {
        vec![UNAVAILABLE.to_owned()]
    } else {
        found
    }
}

#[cfg(test)]
mod tests;
