use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::OnceCell;
use tokio::task::AbortHandle;
use tokio::time::{Instant, sleep};

use crate::admission::Admission;
use crate::capabilities::client_capabilities;
use crate::constants::{EXIT_POLL_MAX_CONSECUTIVE_FAILURES, STOP_POLL_INTERVAL};
use crate::document_path::DocumentPath;
use crate::exit_state::ExitState;
use crate::initialize_result::InitializeResult;
use crate::jsonrpc_connection::JsonRpcConnection;
use crate::launch_command::LaunchCommand;
use crate::listener::Listener;
use crate::lock_ext::LockExt;
use crate::lsp_error::LspError;
use crate::lsp_framer::LspFramer;
use crate::lsp_launcher::LspLauncher;
use crate::lsp_params::{EmptyParams, InitializeParams, WorkspaceFolder};
use crate::lsp_server_options::LspServerOptions;
use crate::lsp_server_spec::LspServerSpec;
use crate::lsp_state::LspState;
use crate::lsp_transport::LspTransport;
use crate::output_chunk::OutputChunk;
use crate::output_kind::OutputKind;
use crate::output_sequencer::OutputSequencer;
use crate::project_dir::ProjectDir;
use crate::runtime::Runtime;
use crate::signal::Signal;
use crate::stderr_handler::StderrHandler;
use crate::utf8_decoder::Utf8Decoder;

pub(crate) struct Inner {
    pub(crate) launcher: Arc<dyn LspLauncher>,
    pub(crate) spec: LspServerSpec,
    pub(crate) project_dir: ProjectDir,
    pub(crate) opts: LspServerOptions,
    pub(crate) state: Mutex<LspState>,
    pub(crate) runtime: Mutex<Option<Runtime>>,
    pub(crate) documents: Mutex<HashMap<String, i32>>,
    pub(crate) stderr_handlers: Mutex<Vec<(u64, StderrHandler)>>,
    pub(crate) listeners: Mutex<Vec<Listener>>,
    pub(crate) next_handler_id: AtomicU64,
    pub(crate) tasks: Mutex<Vec<AbortHandle>>,
    pub(crate) start_lock: tokio::sync::Mutex<()>,
    pub(crate) started: OnceCell<Result<InitializeResult, LspError>>,
    pub(crate) stopped: OnceCell<()>,
}

impl Inner {
    pub(crate) fn state(&self) -> LspState {
        *self.state.locked()
    }

    fn set_state(&self, state: LspState) {
        *self.state.locked() = state;
    }

    /// Moves to `Stopped` only while still active; returns whether it did.
    fn deactivate(&self) -> bool {
        let mut state = self.state.locked();
        let active = state.is_active();
        if active {
            *state = LspState::Stopped;
        }
        active
    }

    pub(crate) fn uri(&self, path: &DocumentPath) -> String {
        self.project_dir.document_uri(path)
    }

    pub(crate) fn running(&self) -> Result<Arc<JsonRpcConnection>, LspError> {
        let state = self.state();
        let runtime = self.runtime.locked();
        match (state.is_active(), runtime.as_ref()) {
            (true, Some(rt)) => Ok(Arc::clone(&rt.connection)),
            _ => Err(LspError::NotRunning(format!(
                "language server is not running (state: {state:?})"
            ))),
        }
    }

    pub(crate) fn abort_tasks(&self) {
        for task in self.tasks.locked().drain(..) {
            task.abort();
        }
    }

    pub(crate) async fn do_start(this: Arc<Self>) -> Result<InitializeResult, LspError> {
        this.set_state(LspState::Starting);
        let result = Self::start_inner(&this).await;
        if result.is_err() {
            let runtime = this.runtime.locked().clone();
            if let Some(rt) = runtime {
                let _ = rt.transport.kill(Signal::Kill).await;
            }
            this.teardown();
        }
        result
    }

    async fn start_inner(this: &Arc<Self>) -> Result<InitializeResult, LspError> {
        let command = LaunchCommand {
            command: this.spec.command.clone(),
            args: this.spec.args.clone(),
            cwd: this.project_dir.clone(),
            env: this.spec.env.clone(),
        };
        let transport = this.launcher.launch(&command).await?;
        let connection = JsonRpcConnection::new(
            Arc::clone(&transport),
            this.opts.request_timeout,
            this.opts.configuration.clone(),
        );
        for listener in this.listeners.locked().iter_mut() {
            listener.attached = Some(connection.on_notification(&listener.method, Arc::clone(&listener.handler)));
        }
        *this.runtime.locked() = Some(Runtime {
            transport: Arc::clone(&transport),
            connection: Arc::clone(&connection),
        });

        let pump = tokio::spawn(Self::pump(
            Arc::clone(this),
            Arc::clone(&transport),
            Arc::clone(&connection),
        ));
        let watch = tokio::spawn(Self::watch_exit(
            Arc::clone(this),
            Arc::clone(&transport),
            Arc::clone(&connection),
        ));
        this.tasks.locked().extend([pump.abort_handle(), watch.abort_handle()]);

        let root_uri = this.project_dir.root_uri();
        let params = InitializeParams {
            process_id: None,
            root_uri: &root_uri,
            workspace_folders: [WorkspaceFolder {
                uri: &root_uri,
                name: this.project_dir.name(),
            }],
            capabilities: client_capabilities(this.opts.capabilities.as_ref())?,
            initialization_options: this.spec.initialization_options.as_ref(),
        };
        let value = connection
            .request("initialize", Some(serde_json::to_value(&params)?), None)
            .await?;
        connection
            .notify("initialized", Some(serde_json::to_value(EmptyParams {})?))
            .await?;
        {
            let mut state = this.state.locked();
            if *state == LspState::Starting {
                *state = LspState::Running;
            }
        }
        serde_json::from_value(value).map_err(|e| LspError::Protocol(format!("unexpected initialize result: {e}")))
    }

    async fn pump(this: Arc<Self>, transport: Arc<dyn LspTransport>, connection: Arc<JsonRpcConnection>) {
        let failure = match transport.output() {
            Ok(output) => this.drain(output, &connection).await,
            Err(err) => Some(err),
        };
        match failure {
            None => {
                if this.deactivate() {
                    connection.close(LspError::NotRunning("language server output stream ended".into()));
                }
            }
            Some(err) => {
                connection.close(err);
                if this.deactivate() {
                    let _ = transport.kill(Signal::Kill).await;
                }
            }
        }
    }

    /// Feeds the server's output to the framer and the connection until the stream ends (`None`)
    /// or something in it is invalid.
    async fn drain(
        &self,
        mut output: BoxStream<'static, Result<OutputChunk, LspError>>,
        connection: &JsonRpcConnection,
    ) -> Option<LspError> {
        let mut framer = LspFramer::new();
        let mut sequencer = OutputSequencer::default();
        let mut stderr = Utf8Decoder::default();
        while let Some(item) = output.next().await {
            let chunk = match item {
                Ok(chunk) => chunk,
                Err(err) => return Some(err),
            };
            match sequencer.admit(&chunk) {
                Err(err) => return Some(err),
                Ok(Admission::Skip) => continue,
                Ok(Admission::Accept) => {}
            }
            match chunk.kind {
                OutputKind::Stderr => self.dispatch_stderr(&stderr.decode(&chunk.data)),
                OutputKind::Stdout => {
                    let handled = framer
                        .push(&chunk.data)
                        .and_then(|bodies| bodies.iter().try_for_each(|body| connection.handle_body(body)));
                    if let Err(err) = handled {
                        return Some(err);
                    }
                }
            }
        }
        None
    }

    fn dispatch_stderr(&self, text: &str) {
        let handlers: Vec<StderrHandler> = self
            .stderr_handlers
            .locked()
            .iter()
            .map(|(_, h)| Arc::clone(h))
            .collect();
        for handler in handlers {
            // a panicking listener must not break the output pump
            let _ = catch_unwind(AssertUnwindSafe(|| handler(text)));
        }
    }

    async fn watch_exit(this: Arc<Self>, transport: Arc<dyn LspTransport>, connection: Arc<JsonRpcConnection>) {
        let mut failures = 0;
        while this.state().is_active() {
            sleep(this.opts.exit_poll_interval).await;
            if !this.state().is_active() {
                return;
            }
            match transport.poll_exit().await {
                Err(_) => {
                    failures += 1;
                    if failures >= EXIT_POLL_MAX_CONSECUTIVE_FAILURES && this.deactivate() {
                        connection.close(LspError::NotRunning(format!(
                            "lost contact with language server: exit polling failed {failures} times in a row"
                        )));
                        return;
                    }
                }
                Ok(ExitState::Running) => failures = 0,
                Ok(ExitState::Exited(code)) => {
                    if this.deactivate() {
                        connection.close(LspError::ServerExited(code));
                    }
                    return;
                }
            }
        }
    }

    pub(crate) async fn do_stop(&self) {
        let _guard = self.start_lock.lock().await;
        let runtime = self.runtime.locked().clone();
        let was_running = {
            let mut state = self.state.locked();
            let running = *state == LspState::Running;
            if running {
                *state = LspState::Stopping;
            }
            running
        };
        if let (true, Some(rt)) = (was_running, runtime) {
            let stop_timeout = self.opts.stop_timeout;
            let _ = rt.connection.request("shutdown", None, Some(stop_timeout)).await;
            let _ = rt.connection.notify("exit", None).await;
            let _ = rt.transport.write(&[], trogon_sandbox_contract::StdinEof::Close).await;
            if !wait_for_exit(rt.transport.as_ref(), stop_timeout).await {
                let _ = rt.transport.kill(Signal::Term).await;
                if !wait_for_exit(rt.transport.as_ref(), self.opts.kill_grace).await {
                    let _ = rt.transport.kill(Signal::Kill).await;
                }
            }
        }
        self.teardown();
    }

    fn teardown(&self) {
        self.set_state(LspState::Stopped);
        self.abort_tasks();
        let connection = self.runtime.locked().as_ref().map(|rt| Arc::clone(&rt.connection));
        if let Some(connection) = connection {
            connection.close(LspError::NotRunning("language server stopped".into()));
        }
        self.documents.locked().clear();
    }
}

async fn wait_for_exit(transport: &dyn LspTransport, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match transport.poll_exit().await {
            Ok(ExitState::Exited(_)) => return true,
            Ok(ExitState::Running) => {}
            Err(_) => return false,
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        sleep(STOP_POLL_INTERVAL.min(remaining)).await;
    }
}
