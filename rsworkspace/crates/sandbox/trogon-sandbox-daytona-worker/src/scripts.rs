//! The shell the worker sends through the toolbox. Every word is quoted, so no request value can end a word early.

use trogon_sandbox_contract::{CommandDeadline, CommandEnv, RunCommandRequest};
use trogon_sandbox_protocol::{SIGNAL_BY_MARKER_SCRIPT, guest_capture_script, resolve_exec_cwd};

use crate::constants::{COMMAND_FILE_PREFIX, ROOT_USER, WORKSPACE_PATH};
use crate::shell::quote;

/// The directory a detached command keeps its frame and its done marker in.
pub(crate) fn command_dir(execution_id: &str) -> String {
    format!("{COMMAND_FILE_PREFIX}{execution_id}")
}

/// The environment line that tags every process of a detached command, so a signal can find them.
pub(crate) fn marker_line(execution_id: &str) -> String {
    format!("{}={execution_id}", CommandEnv::RESERVED_KEY)
}

fn default_cwd(request: &RunCommandRequest) -> String {
    match &request.run_as_user {
        Some(user) if user.as_str() != ROOT_USER => format!("/home/{}", user.as_str()),
        _ => WORKSPACE_PATH.to_owned(),
    }
}

/// The words of the capture invocation: `sh -c <script> sandbox-capture <limit> <cwd> <command...>`.
pub(crate) fn capture_words(
    request: &RunCommandRequest,
    deadline: CommandDeadline,
    execution_id: Option<&str>,
) -> Vec<String> {
    let script = match deadline {
        CommandDeadline::Lease => format!(
            "capture_without_deadline() {{ shift 3; \"$@\"; }}; {}",
            guest_capture_script("capture_without_deadline", "env")
        ),
        CommandDeadline::Timeout => guest_capture_script("timeout", "env"),
    };
    let mut words = vec![
        "sh".to_owned(),
        "-c".to_owned(),
        script,
        "sandbox-capture".to_owned(),
        format!("{}s", request.timeout_seconds.max(1)),
        resolve_exec_cwd(&default_cwd(request), request.cwd.as_deref()),
    ];
    let assignments = request
        .env
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .chain(execution_id.map(marker_line));
    match &request.run_as_user {
        Some(user) => {
            words.extend(["sudo", "-n", "-u", user.as_str(), "env"].map(str::to_owned));
            words.extend(assignments);
        }
        None => words.extend(assignments),
    }
    words.extend(request.command.iter().cloned());
    words
}

pub(crate) fn join_quoted(words: &[String]) -> String {
    words.iter().map(|word| quote(word)).collect::<Vec<_>>().join(" ")
}

/// Starts `capture` in the background, writing its frame under `dir`; the done marker appears once it has finished.
pub(crate) fn detach_command(dir: &str, capture: &str) -> String {
    let frame = quote(&format!("{dir}/frame"));
    let pending = quote(&format!("{dir}/done.pending"));
    let done = quote(&format!("{dir}/done"));
    let inner = format!("{capture} >{frame} 2>/dev/null; printf '%s\\n' \"$?\" >{pending} && mv -- {pending} {done}");
    format!(
        "umask 077; mkdir -m 700 -- {dir} || exit 1; nohup sh -c {inner} >/dev/null 2>&1 </dev/null &",
        dir = quote(dir),
        inner = quote(&inner)
    )
}

/// Prints the frame of a finished detached command, and exits with the running status while it has not finished.
pub(crate) fn poll_script(dir: &str, running_exit: i32) -> String {
    format!(
        "if [ -e {done} ]; then cat -- {frame}; else exit {running_exit}; fi",
        done = quote(&format!("{dir}/done")),
        frame = quote(&format!("{dir}/frame"))
    )
}

pub(crate) fn remove_script(dir: &str) -> String {
    format!("rm -rf -- {}", quote(dir))
}

/// Signals every process that carries `marker`; it exits zero when it signaled at least one.
pub(crate) fn signal_script(marker: &str, signal: u32) -> String {
    join_quoted(&[
        "sh".to_owned(),
        "-c".to_owned(),
        SIGNAL_BY_MARKER_SCRIPT.to_owned(),
        "sandbox-signal".to_owned(),
        marker.to_owned(),
        signal.to_string(),
    ])
}

#[cfg(test)]
mod tests;
