//! ACP client for the terminal REPL — permission and elicitation via `/dev/tty`.
//!
//! See `docs/permission-ui-design.md`.

use agent_client_protocol::{
    Client, CreateTerminalRequest, CreateTerminalResponse, KillTerminalRequest, KillTerminalResponse,
    PermissionOption, PermissionOptionKind, ReadTextFileRequest, ReadTextFileResponse,
    ReleaseTerminalRequest, ReleaseTerminalResponse, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionNotification, TerminalOutputRequest,
    TerminalOutputResponse, WaitForTerminalExitRequest, WaitForTerminalExitResponse,
    WriteTextFileRequest, WriteTextFileResponse,
};
use async_trait::async_trait;
use std::io::{self, BufRead, Read, Write};
use std::sync::{Arc, Mutex};

/// Shared session metadata updated by the REPL (prefix rebind in PR 6).
#[derive(Debug, Clone, Default)]
pub struct ActiveClientState {
    pub session_id: Option<String>,
    pub prefix: String,
    pub allowed_tools: Vec<String>,
}

pub struct TuiClient {
    #[allow(dead_code)]
    state: Arc<Mutex<ActiveClientState>>,
}

impl TuiClient {
    pub fn new(state: Arc<Mutex<ActiveClientState>>) -> Self {
        Self { state }
    }
}

fn not_implemented(method: &str) -> agent_client_protocol::Error {
    agent_client_protocol::Error::new(-32603, format!("{method} not implemented in trogon CLI"))
}

fn open_dev_tty(read: bool, write: bool) -> io::Result<std::fs::File> {
    std::fs::OpenOptions::new().read(read).write(write).open("/dev/tty")
}

fn flush_stderr() {
    let _ = io::stderr().flush();
}

fn read_char_from_dev_tty() -> io::Result<char> {
    use std::os::unix::io::AsRawFd;
    let mut tty = open_dev_tty(true, false)?;
    let fd = tty.as_raw_fd();

    // Save current terminal settings, switch to raw mode so the user's keypress
    // arrives without waiting for Enter, and flush any stale buffered input
    // (e.g. the '\r' / '\n' left over from the previous rustyline readline call).
    let original = unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut t) != 0 {
            return Err(io::Error::last_os_error());
        }
        t
    };
    let mut raw = original;
    unsafe {
        libc::cfmakeraw(&mut raw);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        libc::tcsetattr(fd, libc::TCSAFLUSH, &raw);
        // TCSAFLUSH discards any pending (unread) input, giving us a clean slate.
    }

    let mut buf = [0u8; 1];
    let result = tty.read_exact(&mut buf);

    unsafe {
        libc::tcsetattr(fd, libc::TCSANOW, &original);
    }

    result?;
    Ok(char::from(buf[0]))
}

fn read_line_from_dev_tty(prompt: &str) -> io::Result<String> {
    let mut tty = open_dev_tty(true, true)?;
    write!(tty, "{prompt}")?;
    tty.flush()?;
    let mut line = String::new();
    io::BufReader::new(tty).read_line(&mut line)?;
    Ok(line.trim_end().to_string())
}

fn allow_bypass() -> bool {
    if std::env::var("SUDO_UID").is_ok() || std::env::var("SUDO_USER").is_ok() {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if let Some(rest) = line.strip_prefix("Uid:\t")
                    && let Some(uid) = rest.split_whitespace().next()
                {
                    return uid != "0";
                }
            }
        }
    }
    true
}

fn exit_plan_mode_options(bypass: bool) -> Vec<PermissionOption> {
    let mut options = Vec::new();
    if bypass {
        options.push(PermissionOption::new(
            "bypassPermissions",
            "Yes, and bypass permissions",
            PermissionOptionKind::AllowAlways,
        ));
    }
    options.push(PermissionOption::new(
        "acceptEdits",
        "Yes, and auto-accept edits",
        PermissionOptionKind::AllowAlways,
    ));
    options.push(PermissionOption::new(
        "default",
        "Yes, and manually approve edits",
        PermissionOptionKind::AllowOnce,
    ));
    options.push(PermissionOption::new(
        "plan",
        "No, keep planning",
        PermissionOptionKind::RejectOnce,
    ));
    options
}

fn format_tool_summary(req: &RequestPermissionRequest) -> String {
    let title = req
        .tool_call
        .fields
        .title
        .as_deref()
        .unwrap_or("tool");
    let raw = req
        .tool_call
        .fields
        .raw_input
        .as_ref()
        .map(|v| v.to_string())
        .unwrap_or_default();
    if raw.is_empty() {
        title.to_string()
    } else {
        let snippet: String = raw.chars().take(120).collect();
        format!("{title}  {snippet}")
    }
}

fn selected_outcome(option_id: impl Into<String>) -> agent_client_protocol::Result<RequestPermissionResponse> {
    Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
        SelectedPermissionOutcome::new(option_id.into()),
    )))
}

fn cancelled_outcome() -> agent_client_protocol::Result<RequestPermissionResponse> {
    Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
}

async fn handle_exit_plan_mode_permission() -> agent_client_protocol::Result<RequestPermissionResponse> {
    let options = exit_plan_mode_options(allow_bypass());
    eprintln!();
    eprintln!("Exit plan mode — choose new mode:");
    for (i, opt) in options.iter().enumerate() {
        eprintln!("  {}. {}", i + 1, opt.name);
    }
    flush_stderr();

    let line = match tokio::task::spawn_blocking(|| read_line_from_dev_tty("Choice: ")).await {
        Ok(Ok(l)) => l,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "ExitPlanMode tty read failed");
            return cancelled_outcome();
        }
        Err(e) => {
            tracing::warn!(error = %e, "ExitPlanMode spawn_blocking panicked");
            return cancelled_outcome();
        }
    };

    let idx = line.trim().parse::<usize>().ok().and_then(|n| n.checked_sub(1));
    let Some(opt) = idx.and_then(|i| options.get(i)) else {
        return cancelled_outcome();
    };
    selected_outcome(opt.option_id.0.to_string())
}

async fn handle_tool_permission(req: &RequestPermissionRequest) -> agent_client_protocol::Result<RequestPermissionResponse> {
    let summary = format_tool_summary(req);
    eprintln!();
    eprintln!("┆ {summary}");
    eprintln!("[a]llow  [A]lways allow  [r]eject");
    flush_stderr();

    let key = match tokio::task::spawn_blocking(read_char_from_dev_tty).await {
        Ok(Ok(k)) => k,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "permission tty read failed");
            return cancelled_outcome();
        }
        Err(e) => {
            tracing::warn!(error = %e, "permission spawn_blocking panicked");
            return cancelled_outcome();
        }
    };
    eprintln!();

    match key {
        'a' => selected_outcome("allow"),
        'A' => selected_outcome("allow_always"),
        'r' | 'R' => selected_outcome("reject"),
        '\x03' | '\x04' => cancelled_outcome(),
        _ => selected_outcome("reject"),
    }
}

#[async_trait(?Send)]
impl Client for TuiClient {
    async fn session_notification(&self, _n: SessionNotification) -> agent_client_protocol::Result<()> {
        Ok(())
    }

    async fn request_permission(
        &self,
        req: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        let title = req.tool_call.fields.title.as_deref().unwrap_or("");
        if title == "ExitPlanMode" || req.tool_call.fields.title.as_deref() == Some("Exit Plan Mode") {
            return handle_exit_plan_mode_permission().await;
        }
        handle_tool_permission(&req).await
    }

    async fn read_text_file(&self, _: ReadTextFileRequest) -> agent_client_protocol::Result<ReadTextFileResponse> {
        Err(not_implemented("read_text_file"))
    }

    async fn write_text_file(
        &self,
        _: WriteTextFileRequest,
    ) -> agent_client_protocol::Result<WriteTextFileResponse> {
        Err(not_implemented("write_text_file"))
    }

    async fn create_terminal(
        &self,
        _: CreateTerminalRequest,
    ) -> agent_client_protocol::Result<CreateTerminalResponse> {
        Err(not_implemented("create_terminal"))
    }

    async fn kill_terminal(&self, _: KillTerminalRequest) -> agent_client_protocol::Result<KillTerminalResponse> {
        Err(not_implemented("kill_terminal"))
    }

    async fn terminal_output(
        &self,
        _: TerminalOutputRequest,
    ) -> agent_client_protocol::Result<TerminalOutputResponse> {
        Err(not_implemented("terminal_output"))
    }

    async fn release_terminal(
        &self,
        _: ReleaseTerminalRequest,
    ) -> agent_client_protocol::Result<ReleaseTerminalResponse> {
        Err(not_implemented("release_terminal"))
    }

    async fn wait_for_terminal_exit(
        &self,
        _: WaitForTerminalExitRequest,
    ) -> agent_client_protocol::Result<WaitForTerminalExitResponse> {
        Err(not_implemented("wait_for_terminal_exit"))
    }
}

// Elicitation uses the blanket `ElicitationClient for T` default (Cancel) until acp-nats
// allows specialized overrides. Tty elicitation will be wired when that lands.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_bypass_false_under_sudo() {
        unsafe { std::env::set_var("SUDO_UID", "1000") };
        assert!(!allow_bypass());
        unsafe { std::env::remove_var("SUDO_UID") };
    }

    #[test]
    fn parse_tool_permission_outcome_maps_keys() {
        let allow = RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("allow"));
        let always = RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("allow_always"));
        let reject = RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("reject"));
        assert!(matches!(allow, RequestPermissionOutcome::Selected(_)));
        assert!(matches!(always, RequestPermissionOutcome::Selected(_)));
        assert!(matches!(reject, RequestPermissionOutcome::Selected(_)));
    }

    #[test]
    fn exit_plan_mode_options_include_plan() {
        let opts = exit_plan_mode_options(false);
        assert!(opts.iter().any(|o| o.option_id.0.as_ref() == "plan"));
        assert!(!opts.iter().any(|o| o.option_id.0.as_ref() == "bypassPermissions"));
    }

    #[test]
    fn exit_plan_mode_options_include_bypass_when_allowed() {
        let opts = exit_plan_mode_options(true);
        assert!(opts.iter().any(|o| o.option_id.0.as_ref() == "bypassPermissions"));
    }
}
