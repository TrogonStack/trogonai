//! An in-memory transport for the tests: no NATS, no sandbox.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use trogon_sandbox_contract::StdinEof;
use trogon_sandbox_lsp::{
    ExitState, LaunchCommand, LspError, LspFramer, LspLauncher, LspTransport, OutputChunk, OutputKind, ProjectDir,
    Signal, encode_message,
};

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

type Responder = Box<dyn Fn(&Value) -> Vec<Value> + Send + Sync>;

const OUTPUT_CAPACITY: usize = 1024;

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
struct FakeState {
    framer: LspFramer,
    written: Vec<Value>,
    eof_seen: bool,
    kills: Vec<Signal>,
    exit_script: VecDeque<Result<ExitState, LspError>>,
    exit: Option<ExitState>,
    next_stdout_seq: u64,
    exit_on_eof: bool,
    exit_on_signal: Option<Signal>,
    fail_writes: bool,
    polls: usize,
}

type OutputItem = Result<OutputChunk, LspError>;

pub struct FakeTransport {
    state: Mutex<FakeState>,
    responder: Mutex<Option<Responder>>,
    tx: Mutex<Option<mpsc::Sender<OutputItem>>>,
    rx: Mutex<Option<mpsc::Receiver<OutputItem>>>,
}

impl FakeTransport {
    pub fn new() -> Arc<Self> {
        let (tx, rx) = mpsc::channel(OUTPUT_CAPACITY);
        Arc::new(Self {
            state: Mutex::new(FakeState::default()),
            responder: Mutex::new(None),
            tx: Mutex::new(Some(tx)),
            rx: Mutex::new(Some(rx)),
        })
    }

    /// Answers every message the client writes with the returned messages.
    pub fn respond_with(&self, f: impl Fn(&Value) -> Vec<Value> + Send + Sync + 'static) {
        *lock(&self.responder) = Some(Box::new(f));
    }

    /// A tiny language server: replies to `initialize` and `shutdown`, ignores the rest.
    pub fn respond_to_lifecycle(&self) {
        self.respond_with(|msg| match msg.get("method").and_then(Value::as_str) {
            Some("initialize") => vec![reply(msg, json!({ "capabilities": {} }))],
            Some("shutdown") => vec![reply(msg, Value::Null)],
            _ => vec![],
        });
    }

    pub fn written(&self) -> Vec<Value> {
        lock(&self.state).written.clone()
    }

    pub fn methods(&self) -> Vec<String> {
        self.written()
            .iter()
            .filter_map(|m| m.get("method").and_then(Value::as_str).map(str::to_owned))
            .collect()
    }

    pub fn eof_seen(&self) -> bool {
        lock(&self.state).eof_seen
    }

    pub fn kills(&self) -> Vec<Signal> {
        lock(&self.state).kills.clone()
    }

    pub fn polls(&self) -> usize {
        lock(&self.state).polls
    }

    pub fn exit_with(&self, code: Option<i32>) {
        lock(&self.state).exit = Some(ExitState::Exited(code));
    }

    pub fn script_exit_polls(&self, polls: Vec<Result<ExitState, LspError>>) {
        lock(&self.state).exit_script = polls.into();
    }

    pub fn exit_on_eof(&self) {
        lock(&self.state).exit_on_eof = true;
    }

    pub fn exit_on_signal(&self, signal: Signal) {
        lock(&self.state).exit_on_signal = Some(signal);
    }

    pub fn fail_writes(&self) {
        lock(&self.state).fail_writes = true;
    }

    /// Delivers the server's stdout, framed, under the next stdout seq.
    pub fn emit(&self, message: &Value) -> Result<(), LspError> {
        self.emit_raw(&encode_message(message)?);
        Ok(())
    }

    pub fn emit_raw(&self, bytes: &[u8]) {
        let seq = {
            let mut state = lock(&self.state);
            let seq = state.next_stdout_seq;
            state.next_stdout_seq += 1;
            seq
        };
        self.emit_chunk(OutputKind::Stdout, seq, bytes);
    }

    pub fn emit_chunk(&self, kind: OutputKind, seq: u64, bytes: &[u8]) {
        self.emit_item(Ok(OutputChunk {
            kind,
            seq,
            data: bytes.to_vec(),
        }));
    }

    pub fn emit_error(&self, err: LspError) {
        self.emit_item(Err(err));
    }

    fn emit_item(&self, item: OutputItem) {
        if let Some(tx) = lock(&self.tx).as_ref() {
            let _ = tx.try_send(item);
        }
    }

    /// Ends the output stream, as when the evidence subscription closes.
    pub fn close_output(&self) {
        lock(&self.tx).take();
    }
}

pub fn reply(request: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": request["id"], "result": result })
}

#[async_trait]
impl LspTransport for FakeTransport {
    async fn write(&self, data: &[u8], eof: StdinEof) -> Result<(), LspError> {
        let messages = {
            let mut state = lock(&self.state);
            if state.fail_writes {
                return Err(LspError::Transport("write failed".into()));
            }
            if eof.closes() {
                state.eof_seen = true;
                if state.exit_on_eof {
                    state.exit = Some(ExitState::Exited(Some(0)));
                }
            }
            let bodies = state.framer.push(data)?;
            let messages = bodies
                .iter()
                .map(|body| serde_json::from_str::<Value>(body))
                .collect::<Result<Vec<_>, _>>()?;
            state.written.extend(messages.iter().cloned());
            messages
        };
        let replies: Vec<Value> = match lock(&self.responder).as_ref() {
            Some(responder) => messages.iter().flat_map(responder).collect(),
            None => vec![],
        };
        for message in replies {
            self.emit(&message)?;
        }
        Ok(())
    }

    fn output(&self) -> Result<BoxStream<'static, OutputItem>, LspError> {
        let rx = lock(&self.rx)
            .take()
            .ok_or_else(|| LspError::Transport("output was already taken".into()))?;
        Ok(futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|item| (item, rx)) }).boxed())
    }

    async fn poll_exit(&self) -> Result<ExitState, LspError> {
        let mut state = lock(&self.state);
        state.polls += 1;
        if let Some(scripted) = state.exit_script.pop_front() {
            return scripted;
        }
        Ok(state.exit.clone().unwrap_or(ExitState::Running))
    }

    async fn kill(&self, signal: Signal) -> Result<(), LspError> {
        let mut state = lock(&self.state);
        state.kills.push(signal);
        if state.exit_on_signal == Some(signal) {
            state.exit = Some(ExitState::Exited(None));
        }
        Ok(())
    }
}

pub struct FakeLauncher {
    pub transport: Arc<FakeTransport>,
    pub launched: Mutex<Vec<LaunchCommand>>,
}

impl FakeLauncher {
    pub fn new(transport: Arc<FakeTransport>) -> Arc<Self> {
        Arc::new(Self {
            transport,
            launched: Mutex::new(Vec::new()),
        })
    }

    pub fn launched(&self) -> Vec<LaunchCommand> {
        lock(&self.launched).clone()
    }
}

#[async_trait]
impl LspLauncher for FakeLauncher {
    async fn launch(&self, command: &LaunchCommand) -> Result<Arc<dyn LspTransport>, LspError> {
        lock(&self.launched).push(command.clone());
        Ok(self.transport.clone())
    }
}

/// A launcher that cannot start the process.
pub struct FailingLauncher;

#[async_trait]
impl LspLauncher for FailingLauncher {
    async fn launch(&self, _command: &LaunchCommand) -> Result<Arc<dyn LspTransport>, LspError> {
        Err(LspError::Transport("cannot launch".into()))
    }
}

pub fn project_dir() -> Result<ProjectDir, Box<dyn std::error::Error>> {
    Ok(ProjectDir::new("/workspace/project")?)
}

/// Lets spawned tasks run until `cond` holds.
pub async fn eventually(cond: impl Fn() -> bool) -> TestResult {
    for _ in 0..1000 {
        if cond() {
            return Ok(());
        }
        tokio::task::yield_now().await;
    }
    Err("condition not reached".into())
}

/// A server whose process exits as soon as stdin closes.
pub fn server_for(
    transport: &Arc<FakeTransport>,
) -> Result<(trogon_sandbox_lsp::LspServer, Arc<FakeLauncher>), Box<dyn std::error::Error>> {
    transport.exit_on_eof();
    server_with(transport, trogon_sandbox_lsp::LspServerOptions::default())
}

pub fn server_with(
    transport: &Arc<FakeTransport>,
    opts: trogon_sandbox_lsp::LspServerOptions,
) -> Result<(trogon_sandbox_lsp::LspServer, Arc<FakeLauncher>), Box<dyn std::error::Error>> {
    let launcher = FakeLauncher::new(Arc::clone(transport));
    let server = trogon_sandbox_lsp::LspServer::new(
        launcher.clone(),
        trogon_sandbox_lsp::LspPresetName::TypeScript,
        project_dir()?,
        opts,
    );
    Ok((server, launcher))
}

pub fn doc(path: &str) -> Result<trogon_sandbox_lsp::DocumentPath, Box<dyn std::error::Error>> {
    Ok(trogon_sandbox_lsp::DocumentPath::new(path)?)
}
