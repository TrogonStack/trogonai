//! Terminal presentation helpers for the unified CLI UI.

/// Last dotted segment of the runner prefix (e.g. `acp.claude` → `claude`).
pub fn runner_label(prefix: &str) -> &str {
    prefix.rsplit('.').next().unwrap_or(prefix)
}

/// One-line capability warning shown when the codex runner is the active runner.
///
/// The codex runner is observational-only (RUN-2 descope): it backs onto a
/// `codex app-server` subprocess and provides none of trogon's coding surface —
/// no file/editor tools, no compaction, no sub-agent spawn, and no enforced
/// permission gates (sessions are in-memory). Steer users to a full runner.
pub const CODEX_OBSERVATIONAL_WARNING: &str = "\x1b[33m⚠ codex runner is observational-only: no file tools, compaction, sub-agent spawn, or enforced permissions. Use acp/openrouter/xai for full coding workflows.\x1b[0m";

/// Emit [`CODEX_OBSERVATIONAL_WARNING`] to stderr when `prefix` routes to the
/// codex runner; no-op otherwise. Stderr keeps it out of `--print` stdout.
/// Returns whether the warning fired (used by tests to assert the gating).
pub fn warn_if_codex_observational(prefix: &str) -> bool {
    if runner_label(prefix) == "codex" {
        eprintln!("{CODEX_OBSERVATIONAL_WARNING}");
        true
    } else {
        false
    }
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

/// Dimmed echo of a REPL command (`cd`, `!…`, `/…`). Unlike [`print_user_line`],
/// this is deliberately not styled as a message to the model — it just shows
/// what command ran above its output.
pub fn print_command_echo(line: &str) {
    eprintln!("\x1b[90m{line}\x1b[0m");
}

/// Bold assistant prefix, printed once before an assistant text run.
pub fn print_assistant_prefix() {
    eprint!("\n\x1b[1mTrogon\x1b[0m  ");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_label_takes_last_segment() {
        assert_eq!(runner_label("acp.codex"), "codex");
        assert_eq!(runner_label("acp.claude"), "claude");
        assert_eq!(runner_label("bare"), "bare");
    }

    #[test]
    fn codex_warning_fires_only_for_codex_runner() {
        assert!(warn_if_codex_observational("acp.codex"));
        // Every other runner is a full coding runner — no warning.
        assert!(!warn_if_codex_observational("acp.claude"));
        assert!(!warn_if_codex_observational("acp.grok"));
        assert!(!warn_if_codex_observational("acp.openrouter"));
        assert!(!warn_if_codex_observational("acp.wasm"));
    }

    #[test]
    fn codex_warning_text_names_the_missing_capabilities() {
        assert!(CODEX_OBSERVATIONAL_WARNING.contains("observational-only"));
        assert!(CODEX_OBSERVATIONAL_WARNING.contains("file tools"));
        assert!(CODEX_OBSERVATIONAL_WARNING.contains("compaction"));
    }
}
