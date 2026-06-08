use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::{
    CreateTerminalRequest, CreateTerminalResponse, TerminalOutputRequest,
};
use serde_json::Value;
use trogon_tools::ToolDef;
use trogon_mcp::McpCallTool;

use crate::session_store::{BashJob, SessionStore};

type CreationLockMap = std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>;

fn creation_locks() -> &'static CreationLockMap {
    static LOCKS: std::sync::OnceLock<CreationLockMap> = std::sync::OnceLock::new();
    LOCKS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Returns a per-session async Mutex used to serialize terminal creation.
///
/// Two concurrent bash calls for the same session both find `terminal_id == None`
/// on the first store load; without this lock, both would create a bash process and
/// the first one would be orphaned. Holding this lock across the check-create-save
/// critical section ensures only one terminal is ever created per session.
fn session_creation_lock(session_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    creation_locks()
        .lock()
        .unwrap()
        .entry(session_id.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

/// Drop the per-session creation lock from the global map once the terminal exists.
///
/// B9: the lock was inserted on first use and never removed, so the map grew without
/// bound (one entry per session, forever). It is safe to remove here: a later caller
/// either still holds an `Arc` clone (so the live lock keeps working) or creates a
/// fresh, uncontended lock and immediately reloads the now-saved `terminal_id` —
/// reusing the existing terminal rather than spawning a duplicate. Removing it only
/// after the terminal is confirmed created preserves the creation-race guard.
fn release_session_creation_lock(session_id: &str) {
    creation_locks().lock().unwrap().remove(session_id);
}

pub const DEFAULT_BASH_TIMEOUT_SECS: u64 = 120;
pub const MAX_BASH_TIMEOUT_SECS: u64 = 600;

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const START_MARKER_PREFIX: &str = "__START_";
const START_MARKER_SUFFIX: &str = "__";
const EXIT_MARKER_PREFIX: &str = "__EXIT_";
const EXIT_MARKER_SUFFIX: &str = "__";

/// Monotonically increasing counter used to generate unique start-marker IDs
/// for each bash command invocation. No external dep needed.
static INVOCATION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Implements the `bash` tool by delegating execution to a running `trogon-wasm-runtime`
/// instance discovered via the agent registry.
///
/// Maintains a persistent bash terminal per session — the first call creates it
/// (using `sandbox_dir` as the working directory) and saves its ID in `SessionState`
/// (persisted to NATS KV). Subsequent calls reuse the same terminal via
/// `terminal.write_stdin`, using the demarcation protocol
/// `<command>; echo "__EXIT_$?__"` to detect completion.
pub struct WasmRuntimeBashTool<S> {
    nats: async_nats::Client,
    wasm_prefix: String,
    session_id: String,
    sandbox_dir: PathBuf,
    timeout: Duration,
    store: S,
}

impl<S: SessionStore> WasmRuntimeBashTool<S> {
    pub fn new(
        nats: async_nats::Client,
        wasm_prefix: impl Into<String>,
        session_id: impl Into<String>,
        sandbox_dir: PathBuf,
        timeout: Duration,
        store: S,
    ) -> Self {
        Self {
            nats,
            wasm_prefix: wasm_prefix.into(),
            session_id: session_id.into(),
            sandbox_dir,
            timeout,
            store,
        }
    }

    pub fn tool_def() -> ToolDef {
        ToolDef {
            name: "bash".to_string(),
            description: "Run a shell command in the session sandbox and return its output. \
                Long-running commands are supported (up to 600 seconds via timeout_secs); \
                partial output is returned if the command times out."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute."
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Maximum seconds to wait for the command (default 120, max 600). Use a higher value for long builds/tests."
                    },
                    "run_in_background": {
                        "type": "boolean",
                        "description": "Run the command in the background; returns a job id immediately. Poll output with the bash_output tool. Use for long-running commands you want to monitor."
                    }
                },
                "required": ["command"]
            }),
            cache_control: None,
        }
    }

    pub fn into_dispatch(self) -> (String, String, Arc<dyn McpCallTool>)
    where
        S: Send + Sync + 'static,
    {
        ("bash".to_string(), "bash".to_string(), Arc::new(self))
    }
}

impl<S: SessionStore> McpCallTool for WasmRuntimeBashTool<S> {
    fn call_tool<'a>(
        &'a self,
        _name: &'a str,
        arguments: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        let nats = self.nats.clone();
        let wasm_prefix = self.wasm_prefix.clone();
        let session_id = self.session_id.clone();
        let sandbox_dir = self.sandbox_dir.clone();
        let timeout = resolve_timeout(arguments, self.timeout);
        let store = self.store.clone();

        Box::pin(async move {
            let command = arguments["command"]
                .as_str()
                .ok_or_else(|| "missing command argument".to_string())?
                .to_string();

            let term_base = format!("{wasm_prefix}.session.{session_id}.client.terminal");
            let ext_base = format!("{wasm_prefix}.session.{session_id}.client.ext");

            if arguments["run_in_background"].as_bool() == Some(true) {
                return start_background_job(
                    &nats,
                    &term_base,
                    &ext_base,
                    &session_id,
                    &sandbox_dir,
                    &store,
                    command,
                )
                .await;
            }

            // ── Obtain or create the persistent terminal ──────────────────────
            // The creation lock serialises concurrent bash calls for the same session
            // so that both can't see terminal_id==None and each spawn their own bash
            // process, leaving the loser orphaned. The second caller reloads state
            // inside the lock and reuses the terminal the first caller just saved.
            let terminal_id: String = {
                let _create_lock = session_creation_lock(&session_id).lock_owned().await;

                let mut state = store.load(&session_id).await.map_err(|e| e.to_string())?;
                let cwd_str = sandbox_dir.to_string_lossy().into_owned();
                if state.terminal_id.is_some()
                    && state.terminal_cwd.as_deref() != Some(cwd_str.as_str())
                {
                    state.terminal_id = None;
                    state.terminal_cwd = None;
                }

                let tid = if let Some(tid) = &state.terminal_id {
                    tid.clone()
                } else {
                    let create_req = CreateTerminalRequest::new(session_id.clone(), "bash")
                        .cwd(sandbox_dir.clone());
                    let payload =
                        serde_json::to_vec(&create_req).map_err(|e| e.to_string())?;
                    let msg = nats
                        .request(format!("{term_base}.create"), payload.into())
                        .await
                        .map_err(|e| e.to_string())?;
                    let resp: CreateTerminalResponse =
                        serde_json::from_slice(&msg.payload).map_err(|e| e.to_string())?;
                    let tid = resp.terminal_id.0.to_string();
                    state.terminal_id = Some(tid.clone());
                    state.terminal_cwd = Some(cwd_str);
                    store.save(&session_id, &state).await.map_err(|e| e.to_string())?;
                    tid
                };
                // B9: terminal now exists and its id is persisted; drop the per-session
                // creation lock from the global map so it doesn't accumulate forever.
                // Still inside the held lock + after save, so the creation-race guard
                // is preserved. `_create_lock` (the Arc) keeps the live mutex valid
                // until this scope ends.
                release_session_creation_lock(&session_id);
                tid
            };

            // ── Write command wrapped with unique start + exit markers ────────
            // HIGH-21: Instead of snapshotting baseline output length (which can
            // shrink when a partial UTF-8 sequence at snapshot time completes later),
            // we bracket each command with a unique start marker so we can locate
            // this invocation's output regardless of the total buffer size.
            // MED-28: derive an unguessable per-invocation nonce so a command that
            // echoes a literal "__START_5__" / "__EXIT_0__" cannot be mistaken for
            // our markers. The model never sees these tool-execution-time values
            // (pid / nanoseconds / counter), so it cannot reproduce the nonce.
            let nonce = new_invocation_nonce();
            let (start_marker, exit_marker_prefix) = invocation_markers(&nonce);
            let cmd_with_markers =
                build_cmd_with_markers(&command, &start_marker, &exit_marker_prefix);
            write_terminal_stdin(&nats, &ext_base, &terminal_id, &cmd_with_markers)
                .await?;

            // ── Poll for output until marker found or timeout ─────────────────
            let deadline = tokio::time::Instant::now() + timeout;

            loop {
                tokio::time::sleep(POLL_INTERVAL).await;

                let full_output =
                    fetch_terminal_output(&nats, &term_base, &session_id, &terminal_id).await?;

                if let Some(output) = extract_output(&full_output, &start_marker, &exit_marker_prefix) {
                    return Ok(output);
                }

                if tokio::time::Instant::now() >= deadline {
                    // Return whatever comes after the start marker (if present), or empty.
                    let partial = extract_after_start_marker(&full_output, &start_marker)
                        .unwrap_or_default();
                    return Err(format!(
                        "timeout after {}s. Partial output:\n{partial}",
                        timeout.as_secs()
                    ));
                }
            }
        })
    }
}

/// Polls new output from a background bash job started with `run_in_background`.
pub struct BashOutputTool<S> {
    nats: async_nats::Client,
    wasm_prefix: String,
    session_id: String,
    store: S,
}

impl<S: SessionStore> BashOutputTool<S> {
    pub fn new(
        nats: async_nats::Client,
        wasm_prefix: impl Into<String>,
        session_id: impl Into<String>,
        store: S,
    ) -> Self {
        Self {
            nats,
            wasm_prefix: wasm_prefix.into(),
            session_id: session_id.into(),
            store,
        }
    }

    pub fn tool_def() -> ToolDef {
        ToolDef {
            name: "bash_output".to_string(),
            description: "Read new output from a background bash job; reports whether it is still running or has exited (with exit code)."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The background job id returned by bash"
                    }
                },
                "required": ["id"]
            }),
            cache_control: None,
        }
    }

    pub fn into_dispatch(self) -> (String, String, Arc<dyn McpCallTool>)
    where
        S: Send + Sync + 'static,
    {
        (
            "bash_output".to_string(),
            "bash_output".to_string(),
            Arc::new(self),
        )
    }
}

impl<S: SessionStore> McpCallTool for BashOutputTool<S> {
    fn call_tool<'a>(
        &'a self,
        _name: &'a str,
        arguments: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        let nats = self.nats.clone();
        let wasm_prefix = self.wasm_prefix.clone();
        let session_id = self.session_id.clone();
        let store = self.store.clone();

        Box::pin(async move {
            let job_id = arguments["id"]
                .as_str()
                .ok_or_else(|| "missing id argument".to_string())?;

            let term_base = format!("{wasm_prefix}.session.{session_id}.client.terminal");

            let mut state = store.load(&session_id).await.map_err(|e| e.to_string())?;
            let job_idx = state
                .background_jobs
                .iter()
                .position(|j| j.id == job_id)
                .ok_or_else(|| format!("background job not found: {job_id}"))?;

            let terminal_id = state.background_jobs[job_idx].terminal_id.clone();
            let full_output =
                fetch_terminal_output(&nats, &term_base, &session_id, &terminal_id).await?;

            let poll = poll_background_job_output(&full_output, &state.background_jobs[job_idx]);
            let job = &mut state.background_jobs[job_idx];
            job.read_offset = poll.new_read_offset;
            if poll.finished {
                job.finished = true;
                job.exit_code = poll.exit_code;
            }
            store.save(&session_id, &state).await.map_err(|e| e.to_string())?;

            let status = if poll.finished {
                format!(
                    "\n[job {job_id} exited with code {}]",
                    poll.exit_code.unwrap_or(0)
                )
            } else {
                format!("\n[job {job_id} running]")
            };
            Ok(format!("{}{status}", poll.new_output))
        })
    }
}

async fn start_background_job<S: SessionStore>(
    nats: &async_nats::Client,
    term_base: &str,
    ext_base: &str,
    session_id: &str,
    sandbox_dir: &Path,
    store: &S,
    command: String,
) -> Result<String, String> {
    let nonce = new_invocation_nonce();
    let (start_marker, exit_marker_prefix) = invocation_markers(&nonce);
    let job_id = nonce;

    let create_req = CreateTerminalRequest::new(session_id.to_string(), "bash")
        .cwd(sandbox_dir.to_path_buf());
    let payload = serde_json::to_vec(&create_req).map_err(|e| e.to_string())?;
    let msg = nats
        .request(format!("{term_base}.create"), payload.into())
        .await
        .map_err(|e| e.to_string())?;
    let resp: CreateTerminalResponse =
        serde_json::from_slice(&msg.payload).map_err(|e| e.to_string())?;
    let terminal_id = resp.terminal_id.0.to_string();

    let cmd_with_markers = build_cmd_with_markers(&command, &start_marker, &exit_marker_prefix);
    write_terminal_stdin(nats, ext_base, &terminal_id, &cmd_with_markers).await?;

    let mut state = store.load(session_id).await.map_err(|e| e.to_string())?;
    state.background_jobs.push(BashJob {
        id: job_id.clone(),
        command,
        terminal_id,
        start_marker,
        exit_marker_prefix,
        read_offset: 0,
        finished: false,
        exit_code: None,
    });
    store
        .save(session_id, &state)
        .await
        .map_err(|e| e.to_string())?;

    Ok(format!(
        "Started background job {job_id}. Poll its output with the bash_output tool (id=\"{job_id}\")."
    ))
}

fn new_invocation_nonce() -> String {
    let seq = INVOCATION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:x}{:x}", std::process::id(), nanos, seq)
}

fn invocation_markers(nonce: &str) -> (String, String) {
    (
        format!("{START_MARKER_PREFIX}{nonce}{START_MARKER_SUFFIX}"),
        format!("{EXIT_MARKER_PREFIX}{nonce}_"),
    )
}

/// B8: run the user command as a single, fully-quoted `eval` argument instead of
/// interpolating it raw between the markers.
fn build_cmd_with_markers(command: &str, start_marker: &str, exit_marker_prefix: &str) -> String {
    let escaped_command = shell_single_quote(command);
    format!(
        "echo \"{start_marker}\"; eval {escaped_command}; echo \"{exit_marker_prefix}$?{EXIT_MARKER_SUFFIX}\"\n"
    )
}

async fn write_terminal_stdin(
    nats: &async_nats::Client,
    ext_base: &str,
    terminal_id: &str,
    cmd_with_markers: &str,
) -> Result<(), String> {
    let write_req = serde_json::json!({
        "terminal_id": terminal_id,
        "data": cmd_with_markers.as_bytes()
    });
    let payload = serde_json::to_vec(&write_req).map_err(|e| e.to_string())?;
    nats.request(
        format!("{ext_base}.terminal.write_stdin"),
        payload.into(),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

async fn fetch_terminal_output(
    nats: &async_nats::Client,
    term_base: &str,
    session_id: &str,
    terminal_id: &str,
) -> Result<String, String> {
    let req = TerminalOutputRequest::new(session_id.to_string(), terminal_id.to_string());
    let payload = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
    let msg = nats
        .request(format!("{term_base}.output"), payload.into())
        .await
        .map_err(|e| e.to_string())?;
    let resp: serde_json::Value =
        serde_json::from_slice(&msg.payload).map_err(|e| e.to_string())?;
    terminal_output_from_response(&resp)
}

struct JobPollUpdate {
    new_output: String,
    finished: bool,
    exit_code: Option<i32>,
    new_read_offset: usize,
}

fn poll_background_job_output(full_output: &str, job: &BashJob) -> JobPollUpdate {
    let Some(after_start) = extract_after_start_marker(full_output, &job.start_marker) else {
        return JobPollUpdate {
            new_output: String::new(),
            finished: job.finished,
            exit_code: job.exit_code,
            new_read_offset: job.read_offset,
        };
    };

    if let Some(clean) = find_before_exit_marker(after_start, &job.exit_marker_prefix) {
        let exit_code = parse_exit_code(after_start, &job.exit_marker_prefix);
        JobPollUpdate {
            new_output: slice_from_offset(&clean, job.read_offset),
            finished: true,
            exit_code,
            new_read_offset: clean.len(),
        }
    } else {
        JobPollUpdate {
            new_output: slice_from_offset(after_start, job.read_offset),
            finished: false,
            exit_code: None,
            new_read_offset: after_start.len(),
        }
    }
}

fn slice_from_offset(s: &str, offset: usize) -> String {
    if offset >= s.len() {
        String::new()
    } else {
        s[offset..].to_string()
    }
}

fn parse_exit_code(output: &str, exit_prefix: &str) -> Option<i32> {
    let mut last_code: Option<i32> = None;
    let mut search = output;
    let mut offset = 0;
    while let Some(pos) = search.find(exit_prefix) {
        let abs = offset + pos;
        let after = &output[abs + exit_prefix.len()..];
        if let Some(end) = after.find(EXIT_MARKER_SUFFIX) {
            let code_str = &after[..end];
            if code_str.chars().all(|c| c.is_ascii_digit()) {
                last_code = code_str.parse().ok();
            }
        }
        offset = abs + exit_prefix.len();
        search = &output[offset..];
    }
    last_code
}

/// Resolves the per-call bash timeout from optional `timeout_secs` in `arguments`.
fn resolve_timeout(arguments: &Value, default: Duration) -> Duration {
    let args_timeout = arguments["timeout_secs"].as_u64();
    args_timeout
        .map(|s| s.clamp(1, MAX_BASH_TIMEOUT_SECS))
        .map(Duration::from_secs)
        .unwrap_or(default)
}

/// Wrap `s` in single quotes for safe use as one POSIX-shell word, escaping any
/// embedded single quote as the standard `'\''` sequence (close-quote, escaped
/// quote, reopen-quote). The result is a single argv element no matter what `s`
/// contains, so passing it to `eval '<escaped>'` either runs the exact command or
/// fails with an instant syntax error — it can never leave the shell awaiting input.
fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Parses a `terminal.output` NATS reply. Returns an error immediately when the
/// wasm-runtime dispatcher published a JSON-RPC error (e.g. reply exceeded
/// `max_payload`) instead of leaving the poll loop to time out on empty output.
fn terminal_output_from_response(resp: &Value) -> Result<String, String> {
    if let Some(err) = resp.get("error") {
        let message = err
            .get("message")
            .and_then(|m| m.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| err.to_string());
        return Err(message);
    }
    if let Some(status) = resp.get("status").and_then(|s| s.as_str())
        && status != "success"
        && status != "ok"
    {
        return Err(format!("terminal.output status: {status}"));
    }
    let output = resp
        .get("output")
        .or_else(|| resp.get("result").and_then(|r| r.get("output")))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    Ok(output.to_string())
}

/// Returns the text between the start marker line and the exit marker.
///
/// 1. Locate `start_marker` in `output`. If absent → `None` (still buffering).
/// 2. Skip past the rest of that line (the echo of the start marker itself).
/// 3. In the remaining text, find the last valid `__EXIT_N__` marker and
///    return everything before it (trailing newline stripped).
///
/// This eliminates the baseline-length approach (HIGH-21): the start marker
/// uniquely identifies this invocation's output regardless of buffer size,
/// so a partial UTF-8 sequence completing between snapshot and poll can never
/// cause the guard to stall.
fn extract_output(output: &str, start_marker: &str, exit_prefix: &str) -> Option<String> {
    let after_start = extract_after_start_marker(output, start_marker)?;
    // Now search for the (nonce-scoped) exit marker within the portion after the
    // start marker.
    find_before_exit_marker(after_start, exit_prefix)
}

/// Returns the text that follows the start-marker line, or `None` if the
/// start marker has not appeared yet.
fn extract_after_start_marker<'a>(output: &'a str, start_marker: &str) -> Option<&'a str> {
    let marker_pos = output.find(start_marker)?;
    // Advance past the marker token itself.
    let after_marker = &output[marker_pos + start_marker.len()..];
    // Skip the rest of the line on which the marker was echoed
    // (there may be a trailing '\r' before '\n' in some terminal modes).
    let after_line = match after_marker.find('\n') {
        Some(nl) => &after_marker[nl + 1..],
        None => after_marker,
    };
    Some(after_line)
}

/// Searches `output` for `__EXIT_N__`, returns everything before the last
/// valid marker. Returns `None` if no complete exit marker is present yet.
fn find_before_exit_marker(output: &str, exit_prefix: &str) -> Option<String> {
    // Find the last occurrence so partial writes don't confuse us.
    let mut last_match: Option<usize> = None;
    let mut search = output;
    let mut offset = 0;
    while let Some(pos) = search.find(exit_prefix) {
        let abs = offset + pos;
        let after = &output[abs + exit_prefix.len()..];
        if let Some(end) = after.find(EXIT_MARKER_SUFFIX) {
            let code_str = &after[..end];
            if code_str.chars().all(|c| c.is_ascii_digit()) {
                last_match = Some(abs);
            }
        }
        offset = abs + exit_prefix.len();
        search = &output[offset..];
    }
    last_match.map(|pos| {
        let before = &output[..pos];
        before.strip_suffix('\n').unwrap_or(before).to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── shell_single_quote (B8) ───────────────────────────────────────────────

    #[test]
    fn shell_single_quote_wraps_plain_command() {
        assert_eq!(shell_single_quote("ls -la"), "'ls -la'");
    }

    #[test]
    fn shell_single_quote_escapes_embedded_single_quote() {
        // echo 'hi' → '\''-escaped so it remains one shell word.
        assert_eq!(shell_single_quote("echo 'hi'"), "'echo '\\''hi'\\'''");
    }

    #[test]
    fn shell_single_quote_neutralizes_unbalanced_quote() {
        // An unbalanced double-quote that would hang a raw shell becomes inert
        // text inside the single-quoted eval argument.
        let escaped = shell_single_quote("echo \"unbalanced");
        assert!(escaped.starts_with('\'') && escaped.ends_with('\''));
        assert!(escaped.contains("echo \"unbalanced"));
    }

    // ── terminal_output_from_response ─────────────────────────────────────────

    #[test]
    fn terminal_output_from_response_returns_output_field() {
        let resp = serde_json::json!({ "output": "hello" });
        assert_eq!(
            terminal_output_from_response(&resp).unwrap(),
            "hello"
        );
    }

    #[test]
    fn terminal_output_from_response_errors_on_jsonrpc_error() {
        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "error": {
                "code": -32603,
                "message": "Terminal output too large (1048577 bytes)"
            }
        });
        let err = terminal_output_from_response(&resp).unwrap_err();
        assert!(err.contains("too large"), "got: {err}");
    }

    #[test]
    fn terminal_output_from_response_errors_on_non_success_status() {
        let resp = serde_json::json!({ "status": "failed", "output": "" });
        let err = terminal_output_from_response(&resp).unwrap_err();
        assert!(err.contains("failed"), "got: {err}");
    }

    // ── find_before_exit_marker (low-level helper) ────────────────────────────

    #[test]
    fn find_before_exit_marker_finds_exit_zero() {
        let output = "hello\nworld\n__EXIT_0__\n";
        assert_eq!(
            find_before_exit_marker(output, EXIT_MARKER_PREFIX),
            Some("hello\nworld".to_string())
        );
    }

    #[test]
    fn find_before_exit_marker_finds_nonzero_exit_code() {
        let output = "error output\n__EXIT_1__\n";
        assert_eq!(
            find_before_exit_marker(output, EXIT_MARKER_PREFIX),
            Some("error output".to_string())
        );
    }

    #[test]
    fn find_before_exit_marker_returns_none_when_absent() {
        assert_eq!(find_before_exit_marker("no marker here", EXIT_MARKER_PREFIX), None);
    }

    #[test]
    fn find_before_exit_marker_uses_last_marker() {
        let output = "__EXIT_0__\nmore output\n__EXIT_1__\n";
        let result = find_before_exit_marker(output, EXIT_MARKER_PREFIX).unwrap();
        assert!(result.contains("more output"), "got: {result}");
    }

    #[test]
    fn find_before_exit_marker_ignores_non_numeric_codes() {
        let output = "__EXIT_abc__\nreal output\n__EXIT_0__\n";
        let result = find_before_exit_marker(output, EXIT_MARKER_PREFIX).unwrap();
        assert!(result.contains("real output"), "got: {result}");
    }

    // ── extract_output (start marker + exit marker) ───────────────────────────

    #[test]
    fn extract_output_returns_none_when_start_marker_absent() {
        let output = "some prior output\nhello\n__EXIT_0__\n";
        assert_eq!(extract_output(output, "__START_42__", EXIT_MARKER_PREFIX), None);
    }

    #[test]
    fn extract_output_returns_none_when_exit_marker_not_yet_present() {
        let output = "prior\n__START_1__\ncommand running...\n";
        assert_eq!(extract_output(output, "__START_1__", EXIT_MARKER_PREFIX), None);
    }

    #[test]
    fn extract_output_returns_text_between_markers() {
        let output = "old stuff\n__START_7__\nhello\nworld\n__EXIT_0__\n";
        assert_eq!(
            extract_output(output, "__START_7__", EXIT_MARKER_PREFIX),
            Some("hello\nworld".to_string())
        );
    }

    #[test]
    fn extract_output_ignores_exit_marker_before_start() {
        // An __EXIT__ from a previous command must not be picked up.
        let output = "__EXIT_0__\n__START_3__\nnew cmd\n__EXIT_0__\n";
        assert_eq!(
            extract_output(output, "__START_3__", EXIT_MARKER_PREFIX),
            Some("new cmd".to_string())
        );
    }

    #[test]
    fn extract_output_handles_nonzero_exit() {
        let output = "__START_5__\nerror here\n__EXIT_2__\n";
        assert_eq!(
            extract_output(output, "__START_5__", EXIT_MARKER_PREFIX),
            Some("error here".to_string())
        );
    }

    /// HIGH-21 regression: if a partial UTF-8 sequence (\xc3) at baseline time
    /// completes (\xc3\xa9 = 'é') on the next poll, the total byte count can
    /// shrink. With the start-marker approach the poll loop never uses byte
    /// offsets into the full buffer, so this is a non-issue.
    #[test]
    fn extract_output_works_with_multibyte_chars_before_start() {
        // Simulate "é" (U+00E9, 2 bytes in UTF-8) appearing before the start marker.
        let output = "caf\u{00e9}\n__START_9__\nresult\n__EXIT_0__\n";
        assert_eq!(
            extract_output(output, "__START_9__", EXIT_MARKER_PREFIX),
            Some("result".to_string())
        );
    }

    #[test]
    fn extract_output_ignores_spoofed_exit_marker_with_nonce() {
        // MED-28: a command that prints a literal __EXIT_0__ must not be mistaken
        // for the real, nonce-scoped exit marker. Only the nonce prefix matches.
        let start = "__START_deadbeef__";
        let exit_prefix = "__EXIT_deadbeef_";
        let output = format!(
            "old\n{start}\nfake __EXIT_0__ printed by user\nreal output\n{exit_prefix}0__\n"
        );
        assert_eq!(
            extract_output(&output, start, exit_prefix),
            Some("fake __EXIT_0__ printed by user\nreal output".to_string())
        );
    }

    // ── resolve_timeout ───────────────────────────────────────────────────────

    #[test]
    fn resolve_timeout_uses_default_when_param_absent() {
        let args = serde_json::json!({ "command": "ls" });
        let default = Duration::from_secs(DEFAULT_BASH_TIMEOUT_SECS);
        assert_eq!(resolve_timeout(&args, default), default);
    }

    #[test]
    fn resolve_timeout_honors_explicit_value() {
        let args = serde_json::json!({ "command": "ls", "timeout_secs": 300 });
        assert_eq!(
            resolve_timeout(&args, Duration::from_secs(DEFAULT_BASH_TIMEOUT_SECS)),
            Duration::from_secs(300)
        );
    }

    #[test]
    fn resolve_timeout_clamps_to_max() {
        let args = serde_json::json!({ "command": "ls", "timeout_secs": 99999 });
        assert_eq!(
            resolve_timeout(&args, Duration::from_secs(DEFAULT_BASH_TIMEOUT_SECS)),
            Duration::from_secs(MAX_BASH_TIMEOUT_SECS)
        );
    }

    #[test]
    fn resolve_timeout_clamps_zero_to_one() {
        let args = serde_json::json!({ "command": "ls", "timeout_secs": 0 });
        assert_eq!(
            resolve_timeout(&args, Duration::from_secs(DEFAULT_BASH_TIMEOUT_SECS)),
            Duration::from_secs(1)
        );
    }

    // ── tool_def ──────────────────────────────────────────────────────────────

    #[test]
    fn tool_def_name_is_bash() {
        use crate::session_store::mock::MemorySessionStore;
        let def = WasmRuntimeBashTool::<MemorySessionStore>::tool_def();
        assert_eq!(def.name, "bash");
    }

    #[test]
    fn tool_def_description_is_non_empty() {
        use crate::session_store::mock::MemorySessionStore;
        let def = WasmRuntimeBashTool::<MemorySessionStore>::tool_def();
        assert!(!def.description.is_empty());
    }

    #[test]
    fn tool_def_cache_control_is_none() {
        use crate::session_store::mock::MemorySessionStore;
        let def = WasmRuntimeBashTool::<MemorySessionStore>::tool_def();
        assert!(def.cache_control.is_none());
    }

    #[test]
    fn tool_def_schema_requires_command() {
        use crate::session_store::mock::MemorySessionStore;
        let def = WasmRuntimeBashTool::<MemorySessionStore>::tool_def();
        let required = def.input_schema["required"]
            .as_array()
            .expect("required must be an array");
        assert!(
            required.iter().any(|v| v.as_str() == Some("command")),
            "schema must require 'command', got: {required:?}"
        );
    }

    #[test]
    fn tool_def_schema_command_property_is_string_type() {
        use crate::session_store::mock::MemorySessionStore;
        let def = WasmRuntimeBashTool::<MemorySessionStore>::tool_def();
        let ty = def.input_schema["properties"]["command"]["type"]
            .as_str()
            .expect("command type must be a string");
        assert_eq!(ty, "string");
    }

    // ── background job output polling ─────────────────────────────────────────

    fn sample_job(start: &str, exit_prefix: &str) -> BashJob {
        BashJob {
            id: "job1".to_string(),
            command: "sleep 1".to_string(),
            terminal_id: "term-1".to_string(),
            start_marker: start.to_string(),
            exit_marker_prefix: exit_prefix.to_string(),
            read_offset: 0,
            finished: false,
            exit_code: None,
        }
    }

    #[test]
    fn slice_from_offset_returns_tail_after_offset() {
        assert_eq!(slice_from_offset("hello world", 6), "world");
    }

    #[test]
    fn slice_from_offset_returns_empty_when_offset_past_end() {
        assert_eq!(slice_from_offset("hi", 5), "");
    }

    #[test]
    fn parse_exit_code_reads_last_valid_marker() {
        let output = "noise\n__EXIT_deadbeef_1__\nmore\n__EXIT_deadbeef_0__\n";
        assert_eq!(parse_exit_code(output, "__EXIT_deadbeef_"), Some(0));
    }

    #[test]
    fn parse_exit_code_returns_none_when_absent() {
        assert_eq!(parse_exit_code("still running", "__EXIT_abc_"), None);
    }

    #[test]
    fn poll_background_job_output_returns_empty_when_start_marker_absent() {
        let job = sample_job("__START_1__", "__EXIT_1_");
        let poll = poll_background_job_output("no marker yet", &job);
        assert!(poll.new_output.is_empty());
        assert!(!poll.finished);
        assert_eq!(poll.new_read_offset, 0);
    }

    #[test]
    fn poll_background_job_output_returns_incremental_output_while_running() {
        let job = sample_job("__START_1__", "__EXIT_1_");
        let full = "__START_1__\nline1\nline2\n";
        let poll = poll_background_job_output(full, &job);
        assert_eq!(poll.new_output, "line1\nline2\n");
        assert!(!poll.finished);
        assert_eq!(poll.new_read_offset, "line1\nline2\n".len());
    }

    #[test]
    fn poll_background_job_output_returns_only_new_bytes_since_read_offset() {
        let mut job = sample_job("__START_2__", "__EXIT_2_");
        job.read_offset = 6; // already consumed "line1\n"
        let full = "__START_2__\nline1\nline2\n";
        let poll = poll_background_job_output(full, &job);
        assert_eq!(poll.new_output, "line2\n");
        assert!(!poll.finished);
        assert_eq!(poll.new_read_offset, "line1\nline2\n".len());
    }

    #[test]
    fn poll_background_job_output_detects_exit_and_trims_markers() {
        let job = sample_job("__START_3__", "__EXIT_3_");
        let full = "__START_3__\ndone\n__EXIT_3_2__\n";
        let poll = poll_background_job_output(full, &job);
        assert_eq!(poll.new_output, "done");
        assert!(poll.finished);
        assert_eq!(poll.exit_code, Some(2));
        assert_eq!(poll.new_read_offset, "done".len());
    }

    #[test]
    fn poll_background_job_output_incremental_after_partial_reads_on_finish() {
        let mut job = sample_job("__START_4__", "__EXIT_4_");
        job.read_offset = 5; // already saw "part\n"
        let full = "__START_4__\npart\nial\n__EXIT_4_0__\n";
        let poll = poll_background_job_output(full, &job);
        assert_eq!(poll.new_output, "ial");
        assert!(poll.finished);
        assert_eq!(poll.exit_code, Some(0));
        assert_eq!(poll.new_read_offset, "part\nial".len());
    }

    // ── bash_output tool_def ──────────────────────────────────────────────────

    #[test]
    fn bash_output_tool_def_name_is_bash_output() {
        use crate::session_store::mock::MemorySessionStore;
        let def = BashOutputTool::<MemorySessionStore>::tool_def();
        assert_eq!(def.name, "bash_output");
    }

    #[test]
    fn bash_output_tool_def_schema_requires_id() {
        use crate::session_store::mock::MemorySessionStore;
        let def = BashOutputTool::<MemorySessionStore>::tool_def();
        let required = def.input_schema["required"]
            .as_array()
            .expect("required must be an array");
        assert!(
            required.iter().any(|v| v.as_str() == Some("id")),
            "schema must require 'id', got: {required:?}"
        );
    }
}
