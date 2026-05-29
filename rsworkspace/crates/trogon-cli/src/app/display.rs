//! Terminal presentation helpers for the unified CLI UI.

pub fn runner_label(prefix: &str) -> &str {
    prefix.rsplit('.').next().unwrap_or(prefix)
}

pub fn print_startup_banner(session_id: &str, prefix: &str, permission_mode: &str) {
    let runner = runner_label(prefix);
    eprintln!(
        "\x1b[90mtrogon\x1b[0m · {runner} · {permission_mode} · session {session_id}\x1b[0m"
    );
    eprintln!(
        "\x1b[90m/help\x1b[0m commands · \x1b[90mCtrl+C\x1b[0m cancel · \x1b[90mCtrl+D\x1b[0m quit"
    );
}

pub fn print_user_line(line: &str) {
    eprint!("\x1b[1mYou\x1b[0m  {line}\n");
}

pub fn print_assistant_prefix() {
    eprint!("\n\x1b[1mTrogon\x1b[0m  ");
}
