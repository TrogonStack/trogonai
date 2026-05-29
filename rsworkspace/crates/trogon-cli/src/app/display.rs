//! Terminal presentation helpers for the unified CLI UI.

/// Last dotted segment of the runner prefix (e.g. `acp.claude` → `claude`).
pub fn runner_label(prefix: &str) -> &str {
    prefix.rsplit('.').next().unwrap_or(prefix)
}

/// Startup banner: runner, permission mode, and session id, plus a key-hint line.
pub fn print_startup_banner(session_id: &str, prefix: &str, permission_mode: &str) {
    let runner = runner_label(prefix);
    eprintln!("\x1b[90mtrogon\x1b[0m · {runner} · {permission_mode} · session {session_id}\x1b[0m");
    eprintln!("\x1b[90m/help\x1b[0m commands · \x1b[90mCtrl+C\x1b[0m cancel · \x1b[90mCtrl+D\x1b[0m quit");
}

/// Styled user-input block. The caller erases the readline echo line first when
/// there is one (queued lines have no echo to erase).
pub fn print_user_line(line: &str) {
    eprintln!("\x1b[1mYou\x1b[0m  {line}");
}

/// Bold assistant prefix, printed once before an assistant text run.
pub fn print_assistant_prefix() {
    eprint!("\n\x1b[1mTrogon\x1b[0m  ");
}
