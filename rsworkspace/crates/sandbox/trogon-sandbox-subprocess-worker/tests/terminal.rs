#![cfg(any(target_os = "linux", target_os = "macos"))]

//! A terminal opened through the platform's sandboxing backend, so the
//! pseudo-terminal survives whatever that backend does to the payload.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use serde_json::{Value, json};
use trogon_sandbox_contract::{LeaseId, TerminalId, TerminalOutput, TerminalResponse};
use trogon_sandbox_protocol::terminal_output_bytes;
use trogon_sandbox_subprocess_worker::isolation::IsolationBackend;
use trogon_sandbox_subprocess_worker::isolation::resource_limits::ResourceLimits;
use trogon_sandbox_subprocess_worker::terminal::{FrameSink, SubprocessTerminals};
use trogon_sandbox_subprocess_worker::{EgressPolicy, LaunchSpec};
use trogon_std::env::SystemEnv;

type TestResult = Result<(), Box<dyn Error>>;

const WAIT: Duration = Duration::from_secs(15);

/// The wait for a condition ran out of time.
#[derive(Debug, thiserror::Error)]
#[error("timed out waiting for {what}; output so far: {output:?}")]
struct WaitTimedOutError {
    what: String,
    output: String,
}

#[derive(Default)]
struct Collect {
    frames: Mutex<Vec<TerminalOutput>>,
    changed: tokio::sync::Notify,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[async_trait]
impl FrameSink for Collect {
    async fn publish(&self, subject: String, payload: Bytes) {
        if subject.starts_with("sandbox.terminal.out.")
            && let Ok(frame) = serde_json::from_slice(&payload)
        {
            locked(&self.frames).push(frame);
            self.changed.notify_waiters();
        }
    }
}

impl Collect {
    fn text(&self) -> String {
        let bytes: Vec<u8> = locked(&self.frames)
            .iter()
            .filter_map(terminal_output_bytes)
            .flatten()
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn exited(&self) -> bool {
        locked(&self.frames)
            .iter()
            .any(|frame| matches!(frame, TerminalOutput::Exit { .. }))
    }

    async fn wait_for(&self, what: &str, done: impl Fn(&Self) -> bool) -> Result<(), WaitTimedOutError> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let changed = self.changed.notified();
            if done(self) {
                return Ok(());
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                return Err(WaitTimedOutError {
                    what: what.to_string(),
                    output: self.text(),
                });
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn backend() -> Option<Arc<dyn IsolationBackend>> {
    Some(Arc::new(
        trogon_sandbox_subprocess_worker::isolation::SeatbeltBackend::new(),
    ))
}

#[cfg(target_os = "linux")]
fn backend() -> Option<Arc<dyn IsolationBackend>> {
    let bwrap = which::which("bwrap").ok()?;
    Some(Arc::new(
        trogon_sandbox_subprocess_worker::isolation::BubblewrapBackend::new(bwrap),
    ))
}

/// A terminal request as it arrives on the wire: the operation's fields sit next to the ids.
fn request(op: Value) -> Vec<u8> {
    let mut body = json!({ "lease_id": "l1", "terminal_id": "t1" });
    if let (Some(fields), Value::Object(op)) = (body.as_object_mut(), op) {
        fields.extend(op);
    }
    body.to_string().into_bytes()
}

fn open() -> Value {
    json!({ "op": "open", "cols": 100, "rows": 30, "cmd": [] })
}

fn input(text: &str) -> Value {
    json!({ "op": "input", "data": STANDARD.encode(text) })
}

fn spec_in(root: &Path) -> LaunchSpec {
    let root = root.to_path_buf();
    LaunchSpec {
        binary: PathBuf::from("/bin/sleep"),
        args: vec!["60".to_string()],
        env: vec![("PATH".to_string(), "/usr/bin:/bin".to_string())],
        workspace_host_path: root.clone(),
        workspace_sandbox_path: if cfg!(target_os = "linux") {
            PathBuf::from("/workspace")
        } else {
            root.clone()
        },
        workspace_readonly: false,
        egress_policy: EgressPolicy::DenyAll,
        extra_binds: vec![],
        user: None,
        limits: ResourceLimits::default(),
        fs_policy: None,
        platform: Default::default(),
        fs_denials: None,
    }
}

#[tokio::test]
async fn a_sandboxed_shell_runs_interactively_on_its_terminal() -> TestResult {
    let Some(backend) = backend() else {
        return Ok(());
    };
    let workspace = tempfile::tempdir()?;
    let root = workspace.path().canonicalize()?;
    let sink = Arc::new(Collect::default());
    let terminals = SubprocessTerminals::new(Arc::clone(&sink) as Arc<dyn FrameSink>, &SystemEnv);
    terminals.register_lease(&LeaseId::new("l1"), spec_in(&root), backend);

    assert_eq!(
        terminals.handle(&request(open())).await,
        Some(TerminalResponse::Opened {
            terminal_id: TerminalId::new("t1")?
        })
    );
    let probe = "tty -s && echo tty-ok; stty size; echo file > note; cat note\r";
    assert_eq!(
        terminals.handle(&request(input(probe))).await,
        Some(TerminalResponse::Done)
    );
    sink.wait_for("the probe output", |sink| {
        let text = sink.text();
        text.contains("tty-ok") && text.contains("30 100") && text.contains("\nfile")
    })
    .await?;
    assert_eq!(std::fs::read_to_string(root.join("note"))?, "file\n");

    terminals.handle(&request(input("exit 0\r"))).await;
    sink.wait_for("the exit frame", Collect::exited).await?;
    Ok(())
}

/// A terminal launch keeps its own session, so the namespace's
/// `--new-session` cannot be what stops a payload from pushing input into a
/// terminal; the seccomp filter has to.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_terminal_payload_cannot_inject_input_with_tiocsti() -> TestResult {
    let Some(backend) = backend() else {
        return Ok(());
    };
    if which::which("python3").is_err() {
        return Ok(());
    }
    let workspace = tempfile::tempdir()?;
    let root = workspace.path().canonicalize()?;
    std::fs::write(
        root.join("sti.py"),
        "import fcntl, termios\ntry:\n    fcntl.ioctl(0, termios.TIOCSTI, b'x')\n    print('sti-ok')\nexcept OSError as e:\n    print('sti-errno', e.errno)\n",
    )?;
    let sink = Arc::new(Collect::default());
    let terminals = SubprocessTerminals::new(Arc::clone(&sink) as Arc<dyn FrameSink>, &SystemEnv);
    terminals.register_lease(&LeaseId::new("l1"), spec_in(&root), backend);
    terminals.handle(&request(open())).await;
    terminals.handle(&request(input("python3 /workspace/sti.py\r"))).await;
    sink.wait_for("the probe verdict", |sink| {
        let text = sink.text();
        text.contains("sti-ok") || text.contains("sti-errno")
    })
    .await?;
    let text = sink.text();
    assert!(text.contains("sti-errno 1"), "TIOCSTI not refused: {text:?}");

    terminals.handle(&request(input("exit 0\r"))).await;
    sink.wait_for("the exit frame", Collect::exited).await?;
    Ok(())
}
