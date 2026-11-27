//! The shell the worker sends through the toolbox. Every word is quoted, so no request value can end a word early.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use trogon_sandbox_contract::{RunCommandRequest, StdinMode};
use trogon_sandbox_protocol::{StdinWrite, resolve_exec_cwd};

use crate::constants::{ROOT_USER, STDIN_EOF_LINE, WORKSPACE_PATH};
use crate::shell::quote;

/// The default directory of a per-call user, which is also where that user's commands start.
pub(crate) fn home_of(user: &str) -> String {
    if user == ROOT_USER {
        "/root".to_owned()
    } else {
        format!("/home/{user}")
    }
}

/// Renders argv, environment and directory as one shell command line, since the toolbox takes a command string
/// rather than an argv.
pub(crate) fn render_command(command: &[String], cwd: Option<&str>, env: &[(&str, &str)]) -> String {
    let mut line = String::new();
    if let Some(cwd) = cwd {
        line.push_str(&format!("cd {} && ", quote(cwd)));
    }
    if !env.is_empty() {
        line.push_str("env");
        for (name, value) in env {
            line.push_str(&format!(" {name}={}", quote(value)));
        }
        line.push(' ');
    }
    let words: Vec<String> = command.iter().map(|word| quote(word)).collect();
    line.push_str(&words.join(" "));
    line
}

/// Wraps a rendered line so it runs as `user`. The toolbox has no user field, so the switch happens in the shell:
/// `su` when the sandbox runs as root, passwordless `sudo` otherwise.
pub(crate) fn render_as_user(user: &str, line: &str) -> String {
    let line = quote(line);
    let user = quote(user);
    format!(
        "if [ \"$(id -u)\" = 0 ]; then su -s /bin/sh {user} -c {line}; \
         else sudo -n -u {user} /bin/sh -c {line}; fi"
    )
}

/// The whole request as one command line: argv, environment, directory and the optional per-call user.
pub(crate) fn render_request(request: &RunCommandRequest) -> String {
    let user = request.run_as_user.as_ref().map(|user| user.as_str());
    let cwd = resolve_exec_cwd(
        &user.map_or_else(|| WORKSPACE_PATH.to_owned(), home_of),
        request.cwd.as_deref(),
    );
    let env: Vec<(&str, &str)> = request
        .env
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let line = render_command(&request.command, Some(&cwd), &env);
    match user {
        Some(user) => render_as_user(user, &line),
        None => line,
    }
}

/// Sends the streams of `line` to files, since a toolbox reply and a session log carry one merged stream.
pub(crate) fn capture_output(line: &str, out_path: &str, err_path: &str) -> String {
    format!("( {line} ) >{} 2>{}", quote(out_path), quote(err_path))
}

/// Runs `line` under `timeout`, then records in `status_path` whether the deadline ended it.
///
/// The receipt is `completed` or `timed_out` when the guard ran the command, `unknown` when it never armed, and
/// `unavailable` when the image has no `timeout`.
pub(crate) fn with_deadline(line: &str, timeout_seconds: u32, status_path: &str, stdin: StdinMode) -> String {
    let timeout_seconds = timeout_seconds.max(1);
    let inner = r#"s=$1; c=
if [ "$2" = interactive ]; then exec 3<&0; else exec 3</dev/null; fi
f() { printf timed_out >"$s"; if [ -n "$c" ]; then kill -TERM "$c" 2>/dev/null; { sleep 1; kill -KILL "$c" 2>/dev/null; } & fi; }
trap f TERM
: >"$s.armed" || exit 125
/bin/sh -c "$3" <&3 & c=$!
if [ -e "$s" ]; then kill -TERM "$c" 2>/dev/null; fi
wait "$c"; r=$?
while kill -0 "$c" 2>/dev/null; do wait "$c"; r=$?; done
exit "$r""#;
    let mode = if stdin == StdinMode::Interactive {
        "interactive"
    } else {
        "closed"
    };
    format!(
        "s={}; \
         if ! command -v timeout >/dev/null 2>&1; then printf unavailable >\"$s\"; exit 125; fi; \
         timeout -k 1s {timeout_seconds}s /bin/sh -c {} sandbox-deadline \"$s\" {mode} {}; r=$?; \
         if [ ! -e \"$s\" ]; then \
           if [ -e \"$s.armed\" ]; then printf completed >\"$s\"; else printf unknown >\"$s\"; fi; \
         fi; rm -f \"$s.armed\"; exit \"$r\"",
        quote(status_path),
        quote(inner),
        quote(line),
    )
}

/// Runs `line` with a stdin that a session's input calls feed and that can be closed.
///
/// The toolbox only takes input as text lines and has no call that closes a command's stdin. So each write arrives
/// as one base64 line that a reader decodes into a pipe the command reads, and [`STDIN_EOF_LINE`] ends the reader,
/// which closes the pipe. The reader takes the session's input from fd 3 because a background job's stdin is
/// `/dev/null` in a shell without job control.
pub(crate) fn interactive_stdin(line: &str, fifo: &str) -> String {
    let script = format!(
        "exec 3<&0; f={} ; mkfifo \"$f\" || exit 125; \
         ( while IFS= read -r l; do [ \"$l\" = {STDIN_EOF_LINE} ] && break; \
         printf %s \"$l\" | base64 -d || exit 1; done > \"$f\" ) <&3 & rp=$!; \
         {line} < \"$f\"; rc=$?; kill $rp 2>/dev/null; rm -f \"$f\"; exit $rc",
        quote(fifo),
    );
    format!("/bin/sh -c {}", quote(&script))
}

/// A line whose stdin is closed from the start, so it never waits on the session's own input.
pub(crate) fn closed_stdin(line: &str) -> String {
    format!("( {line} ) </dev/null")
}

/// The input lines one stdin write sends: its data as one base64 line, then the end marker when it closes stdin.
pub(crate) fn stdin_lines(write: &StdinWrite) -> Vec<String> {
    let mut lines = Vec::new();
    if !write.data.as_slice().is_empty() {
        lines.push(STANDARD.encode(write.data.as_slice()));
    }
    if write.eof.closes() {
        lines.push(STDIN_EOF_LINE.to_owned());
    }
    lines
}

#[cfg(test)]
mod tests;
