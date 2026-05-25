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
use crate::terminal::reset_display;
use std::io::{self, BufRead, Write};
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Shared session metadata updated by the REPL (prefix rebind in PR 6).
#[derive(Debug, Clone, Default)]
pub struct ActiveClientState {
    pub session_id: Option<String>,
    pub prefix: String,
    pub allowed_tools: Vec<String>,
}

/// Coordinates permission prompts so only one `/dev/tty` reader is active and stale
/// readers exit when the REPL takes over for `readline`.
#[derive(Debug)]
pub struct PermissionCoordinator {
    cancel: AtomicBool,
    generation: AtomicU64,
}

impl PermissionCoordinator {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            cancel: AtomicBool::new(false),
            generation: AtomicU64::new(0),
        })
    }

    /// Cancel any in-flight permission reader, drain queued tty bytes, reset display.
    pub fn cancel_pending(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        drain_dev_tty();
        drain_stdin();
        reset_display();
        self.cancel.store(false, Ordering::SeqCst);
    }

    /// Begin a new permission prompt; cancels any previous reader and returns its generation.
    ///
    /// Does not drain `/dev/tty` — keys typed right after the prompt appears must not be eaten.
    pub fn begin_prompt(&self) -> u64 {
        self.cancel.store(true, Ordering::SeqCst);
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.cancel.store(false, Ordering::SeqCst);
        generation
    }

    pub fn is_cancelled(&self, generation: u64) -> bool {
        self.cancel.load(Ordering::SeqCst)
            || self.generation.load(Ordering::SeqCst) != generation
    }
}

impl Default for PermissionCoordinator {
    fn default() -> Self {
        Self {
            cancel: AtomicBool::new(false),
            generation: AtomicU64::new(0),
        }
    }
}

pub struct TuiClient {
    #[allow(dead_code)]
    state: Arc<Mutex<ActiveClientState>>,
    coordinator: Arc<PermissionCoordinator>,
}

impl TuiClient {
    pub fn new(state: Arc<Mutex<ActiveClientState>>, coordinator: Arc<PermissionCoordinator>) -> Self {
        Self { state, coordinator }
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

fn tty_debug(msg: impl std::fmt::Display) {
    if std::env::var("TROGON_TTY_DEBUG").is_ok() {
        eprintln!("[TTY-DBG] {msg}");
    }
}

/// Only one permission prompt may read `/dev/tty` at a time (concurrent NATS handlers).
fn permission_prompt_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PermissionKey {
    AllowOnce,
    AllowAlways,
    Reject,
    Cancel,
}

const PERMISSION_TIMEOUT: Duration = Duration::from_secs(55);
const ESCAPE_FOLLOW_MS: i32 = 50;
const POLL_SLICE_MS: i32 = 200;
const INVALID_KEY_DEBOUNCE: Duration = Duration::from_millis(750);

fn read_byte(fd: i32) -> io::Result<Option<u8>> {
    let mut buf = [0u8; 1];
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, 1) };
        if n == 1 {
            return Ok(Some(buf[0]));
        }
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        return Ok(None);
    }
}

fn poll_readable(fd: i32, timeout_ms: i32) -> bool {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
    ready > 0 && pfd.revents & libc::POLLIN != 0
}

fn read_byte_with_timeout(fd: i32, timeout_ms: i32) -> io::Result<Option<u8>> {
    if !poll_readable(fd, timeout_ms) {
        return Ok(None);
    }
    read_byte(fd)
}

/// Discard bytes until a CSI / OSC sequence ends (letter, `~`, or BEL).
fn consume_escape_sequence(fd: i32) {
    loop {
        match read_byte(fd) {
            Ok(Some(b)) if b.is_ascii_alphabetic() || b == b'~' || b == 0x07 => break,
            Ok(Some(_)) => continue,
            _ => break,
        }
    }
}

/// Read a CSI sequence after ESC `[` was already consumed.
fn read_csi_sequence(fd: i32) -> Vec<u8> {
    let mut params = Vec::new();
    loop {
        match read_byte(fd) {
            Ok(Some(b)) if b.is_ascii_alphabetic() || b == b'~' => {
                params.push(b);
                break;
            }
            Ok(Some(b)) => params.push(b),
            _ => break,
        }
    }
    params
}

/// Skip bracketed-paste payload until ESC `[201~`.
fn skip_bracketed_paste_content(fd: i32) {
    loop {
        match read_byte(fd) {
            Ok(Some(0x1b)) => {
                if poll_readable(fd, ESCAPE_FOLLOW_MS)
                    && read_byte(fd).ok().flatten() == Some(b'[')
                {
                    let seq = read_csi_sequence(fd);
                    if is_bracketed_paste_end(&seq) {
                        break;
                    }
                }
            }
            Ok(Some(_)) => continue,
            _ => break,
        }
    }
}

/// Handle ESC and following bytes (CSI, bracketed paste, etc.).
fn consume_after_escape(fd: i32) {
    match read_byte_with_timeout(fd, ESCAPE_FOLLOW_MS) {
        Ok(None) => {}
        Ok(Some(b'[')) => {
            let seq = read_csi_sequence(fd);
            if is_bracketed_paste_start(&seq) {
                skip_bracketed_paste_content(fd);
            }
        }
        Ok(Some(_)) => consume_escape_sequence(fd),
        Err(_) => {}
    }
}

/// Drop any bytes already queued on `fd` (non-blocking poll).
pub(crate) fn drain_available(fd: i32) {
    let mut buf = [0u8; 64];
    loop {
        if !poll_readable(fd, 0) {
            break;
        }
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n <= 0 {
            break;
        }
    }
}

fn drain_dev_tty() {
    if let Ok(tty) = open_dev_tty(true, false) {
        drain_available(tty.as_raw_fd());
    }
}

fn drain_stdin() {
    let fd = io::stdin().as_raw_fd();
    drain_available(fd);
}

fn classify_permission_byte(byte: u8) -> Option<PermissionKey> {
    match byte {
        b'a' => Some(PermissionKey::AllowOnce),
        b'w' | b'W' => Some(PermissionKey::AllowAlways),
        b'r' | b'R' => Some(PermissionKey::Reject),
        0x03 | 0x04 => Some(PermissionKey::Cancel),
        _ => None,
    }
}

/// Returns true when `seq` is the bracketed-paste start marker `[200~`.
pub(crate) fn is_bracketed_paste_start(seq: &[u8]) -> bool {
    seq == [b'2', b'0', b'0', b'~']
}

/// Returns true when `seq` is the bracketed-paste end marker `[201~`.
pub(crate) fn is_bracketed_paste_end(seq: &[u8]) -> bool {
    seq == [b'2', b'0', b'1', b'~']
}

/// Read a single permission decision from `/dev/tty`.
///
/// Uses a process-wide lock so concurrent permission NATS handlers cannot steal
/// each other's keypresses. Respects [`PermissionCoordinator`] cancellation from
/// the REPL and enforces a wall-clock timeout.
fn read_permission_key(coordinator: &PermissionCoordinator) -> io::Result<PermissionKey> {
    let _guard = permission_prompt_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let generation = coordinator.begin_prompt();

    let tty = open_dev_tty(true, true)?;
    let fd = tty.as_raw_fd();

    let original = unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut t) != 0 {
            return Err(io::Error::last_os_error());
        }
        t
    };

    /// RAII guard that restores the terminal to its original `termios` settings on drop.
    struct RawModeGuard {
        fd: i32,
        original: libc::termios,
    }

    impl Drop for RawModeGuard {
        fn drop(&mut self) {
            unsafe {
                libc::tcsetattr(self.fd, libc::TCSANOW, &self.original);
            }
            drain_stdin();
            reset_display();
        }
    }

    unsafe {
        let mut raw = original;
        libc::cfmakeraw(&mut raw);
        raw.c_cc[libc::VMIN] = 0;
        raw.c_cc[libc::VTIME] = 0;
        if libc::tcsetattr(fd, libc::TCSANOW, &raw) != 0 {
            return Err(io::Error::last_os_error());
        }
    }

    // Guard installed after raw mode is set; Drop restores original termios regardless
    // of whether the function returns Ok or Err (covers all `?`-propagated errors).
    let _guard = RawModeGuard { fd, original };

    tty_debug("waiting for permission key…");
    let deadline = Instant::now() + PERMISSION_TIMEOUT;
    let mut last_invalid_msg: Option<Instant> = None;

    loop {
        if coordinator.is_cancelled(generation) {
            tty_debug("permission read cancelled by coordinator");
            break Ok(PermissionKey::Cancel);
        }
        if Instant::now() >= deadline {
            tty_debug("permission read timed out");
            break Ok(PermissionKey::Cancel);
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        let timeout_ms = remaining.as_millis().min(POLL_SLICE_MS as u128) as i32;

        match read_byte_with_timeout(fd, timeout_ms)? {
            None => continue,
            Some(b'\r') | Some(b'\n') => {
                tty_debug("permission read cancelled by Enter");
                break Ok(PermissionKey::Cancel);
            }
            Some(0x1b) => {
                consume_after_escape(fd);
                continue;
            }
            Some(byte) => {
                if let Some(key) = classify_permission_byte(byte) {
                    tty_debug(format!("permission key {byte} -> {key:?}"));
                    break Ok(key);
                }
                tty_debug(format!("ignored byte {byte}"));
                let now = Instant::now();
                if last_invalid_msg.is_none_or(|t| now.duration_since(t) >= INVALID_KEY_DEBOUNCE) {
                    eprintln!("Invalid key — press [a], [w], or [r]");
                    flush_stderr();
                    last_invalid_msg = Some(now);
                }
                continue;
            }
        }
    }
}

fn read_line_from_dev_tty(prompt: &str) -> io::Result<String> {
    let _guard = permission_prompt_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut tty = open_dev_tty(true, true)?;
    write!(tty, "{prompt}")?;
    tty.flush()?;
    let mut line = String::new();
    io::BufReader::new(tty).read_line(&mut line)?;
    drain_stdin();
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

fn permission_key_to_outcome(key: PermissionKey) -> Option<RequestPermissionOutcome> {
    match key {
        PermissionKey::AllowOnce => Some(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new("allow"),
        )),
        PermissionKey::AllowAlways => Some(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new("allow_always"),
        )),
        PermissionKey::Reject => Some(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new("reject"),
        )),
        PermissionKey::Cancel => Some(RequestPermissionOutcome::Cancelled),
    }
}

async fn handle_exit_plan_mode_permission(
    coordinator: Arc<PermissionCoordinator>,
) -> agent_client_protocol::Result<RequestPermissionResponse> {
    let options = exit_plan_mode_options(allow_bypass());
    eprint!("\r\x1b[2K");
    eprintln!();
    eprintln!("Exit plan mode — choose new mode:");
    for (i, opt) in options.iter().enumerate() {
        eprintln!("  {}. {}", i + 1, opt.name);
    }
    flush_stderr();

    // Cancel any racing permission reader before we take the lock ourselves.
    coordinator.begin_prompt();

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

async fn handle_tool_permission(
    req: &RequestPermissionRequest,
    coordinator: Arc<PermissionCoordinator>,
) -> agent_client_protocol::Result<RequestPermissionResponse> {
    // When there is no controlling terminal (CI, piped input) we cannot display
    // the interactive prompt.  If TROGON_NON_INTERACTIVE=1 is set the caller
    // explicitly opts in to auto-approving all tool requests; otherwise we deny
    // with a clear explanation so the user knows to either run interactively or
    // set TROGON_MODE=bypassPermissions / TROGON_NON_INTERACTIVE=1.
    if let Err(e) = open_dev_tty(true, true) {
        if std::env::var("TROGON_NON_INTERACTIVE").as_deref() == Ok("1") {
            tracing::info!(
                tool = req.tool_call.fields.title.as_deref().unwrap_or("<unknown>"),
                "TROGON_NON_INTERACTIVE=1 — auto-approving tool call (no TTY)"
            );
            return selected_outcome("allow");
        }
        tracing::warn!(
            error = %e,
            tool = req.tool_call.fields.title.as_deref().unwrap_or("<unknown>"),
            "no TTY available — tool call denied; set TROGON_NON_INTERACTIVE=1 or TROGON_MODE=bypassPermissions for headless use"
        );
        return cancelled_outcome();
    }

    let summary = format_tool_summary(req);
    reset_display();
    eprint!("\r\x1b[2K");
    eprintln!();
    eprintln!("┆ {summary}");
    eprintln!("[a] allow  [w] always allow  [r] reject");
    flush_stderr();

    let key = match tokio::task::spawn_blocking(move || read_permission_key(&coordinator)).await {
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

    Ok(RequestPermissionResponse::new(
        permission_key_to_outcome(key).unwrap_or(RequestPermissionOutcome::Cancelled),
    ))
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
            return handle_exit_plan_mode_permission(self.coordinator.clone()).await;
        }
        handle_tool_permission(&req, self.coordinator.clone()).await
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
    fn classify_permission_key_bytes() {
        assert_eq!(classify_permission_byte(b'a'), Some(PermissionKey::AllowOnce));
        assert_eq!(classify_permission_byte(b'w'), Some(PermissionKey::AllowAlways));
        assert_eq!(classify_permission_byte(b'R'), Some(PermissionKey::Reject));
        assert_eq!(classify_permission_byte(b'h'), None);
        assert_eq!(classify_permission_byte(b'['), None);
    }

    #[test]
    fn permission_key_maps_to_outcome() {
        assert!(matches!(
            permission_key_to_outcome(PermissionKey::AllowOnce),
            Some(RequestPermissionOutcome::Selected(_))
        ));
        assert!(matches!(
            permission_key_to_outcome(PermissionKey::Cancel),
            Some(RequestPermissionOutcome::Cancelled)
        ));
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

    #[test]
    fn bracketed_paste_markers() {
        assert!(is_bracketed_paste_start(&[b'2', b'0', b'0', b'~']));
        assert!(!is_bracketed_paste_start(&[b'2', b'0', b'1', b'~']));
        assert!(is_bracketed_paste_end(&[b'2', b'0', b'1', b'~']));
        assert!(!is_bracketed_paste_end(&[b'2', b'0', b'0', b'~']));
    }

    #[test]
    fn coordinator_cancel_increments_generation() {
        let coord = PermissionCoordinator::new();
        let gen0 = coord.begin_prompt();
        coord.cancel_pending();
        let gen1 = coord.begin_prompt();
        assert!(gen1 > gen0);
        assert!(coord.is_cancelled(gen0));
        assert!(!coord.is_cancelled(gen1));
    }

    #[test]
    fn coordinator_begin_cancels_previous_generation() {
        let coord = PermissionCoordinator::new();
        let old = coord.begin_prompt();
        let new = coord.begin_prompt();
        assert!(coord.is_cancelled(old));
        assert!(!coord.is_cancelled(new));
    }
}
