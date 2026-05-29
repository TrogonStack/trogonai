//! Terminal presentation helpers for the CLI UI.

/// Startup banner printed once per session.
pub fn print_startup_banner(session_id: &str) {
    eprintln!("trogon — session {session_id} (Ctrl+D to quit)");
}

/// Styled user-input block. The caller erases the readline echo line first when
/// there is one (queued lines have no echo to erase).
pub fn print_user_line(line: &str) {
    eprintln!("\x1b[1;35m┃\x1b[0m {line}");
}
