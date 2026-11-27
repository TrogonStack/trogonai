use trogon_sandbox_contract::{CommandExecution, RunCommandRequest};

fn valid_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// Options Daytona cannot honor are refused rather than silently dropped.
pub(crate) fn run_command_violation(request: &RunCommandRequest) -> Option<&'static str> {
    if request.require_launch_cancellation.is_required() {
        return Some("this worker cannot make a command launch cancelable");
    }
    if request.command.is_empty() {
        return Some("command must not be empty");
    }
    if request.stream_output.is_enabled() {
        return Some("this worker cannot stream command output");
    }
    if let CommandExecution::Detached { process: Some(_), .. } = &request.execution {
        return Some("named processes and readiness probes are not supported");
    }
    if request.env.iter().any(|(name, _)| !valid_env_name(name)) {
        return Some("environment names are letters, digits and underscores, and do not start with a digit");
    }
    None
}

#[cfg(test)]
mod tests;
