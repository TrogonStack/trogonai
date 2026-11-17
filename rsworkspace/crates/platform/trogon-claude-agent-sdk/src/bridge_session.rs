//! `attachBridgeSession` (offset 1197146): the worker client, the read stream, the inbound router and the control
//! dispatch of one remote session, wired together the way the upstream entrypoint wires them.
//!
//! The crate keeps time out of its pieces, so this session does too: the upstream timers (the heartbeat schedule,
//! the uploader's flush timers and the reconnect loop of the read stream) are steps the caller drives. A caller
//! opens the stream with [`BridgeSession::stream_request`], hands the status to [`BridgeSession::stream_opened`] and
//! the body to [`BridgeSession::stream_push`], reopens after [`BridgeSession::stream_lost`], beats with
//! [`BridgeSession::heartbeat`], and sends what is queued with [`BridgeSession::pump`]. Replies the control dispatch
//! writes, and the events the handle writes, wait in the queues until then.
//!
//! Draft, pending upstream call-graph evidence: with `outboundOnly` the read stream is never opened, so no request
//! arrives to be refused (`FPe`, offset 1190500), and the refusal of a process that is exiting (`G4`) has no way to
//! be set from the attach options.

use std::ffi::OsStr;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::SensitiveString;
use crate::adapters::{EnvironmentVariable, HttpExecute, SystemEnvironment};
use crate::bridge_session_callbacks::{CloseCallback, InboundMessageCallback, PermissionResponseCallback};
use crate::bridge_session_core::{Core, Runtime};
use crate::bridge_session_runtime::{Lifecycle, Uploads, spawn_uploads, wind_down};
use crate::bridge_worker::{InvalidSessionIdError, TokioPacing, WorkerClient, WorkerConfig, WorkerPacing};
use crate::bridge_worker_control::ControlCallbacks;
use crate::bridge_worker_control_async::{AsyncControl, AsyncControlCallbacks, background_tasks_disabled};
use crate::bridge_worker_heartbeat::HeartbeatDriver;
use crate::bridge_worker_inbound::InboundRouter;
use crate::bridge_worker_stream::{Reopen, StreamOpen, StreamRequest, StreamSession};
use crate::bridge_worker_stream_driver::{StreamClocks, run_stream};
use crate::bridge_worker_stream_http::HttpStreamConnector;
use crate::bridge_worker_types::{InitError, MismatchReason, RegisterFailureError};

mod constants;

pub use constants::CLOSE_INIT_FAILED;

/// What the host reads the environment with (`process.env`).
pub type EnvironmentLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The host callbacks of `AttachBridgeSessionOptions`.
#[derive(Default)]
pub struct AttachCallbacks {
    pub on_inbound_message: Option<InboundMessageCallback>,
    pub on_permission_response: Option<PermissionResponseCallback>,
    pub on_close: Option<CloseCallback>,
    /// `onInterrupt`, `onSetModel`, `onSetMaxThinkingTokens`, `onSetPermissionMode` and `onRenameSession`. The
    /// `has_*` flags are set from [`AttachCallbacks::asynchronous`].
    pub control: ControlCallbacks,
    /// `onStopTask`, `onBackgroundTasks` and `onSideQuestion`.
    pub asynchronous: AsyncControlCallbacks,
}

/// `AttachBridgeSessionOptions`.
pub struct AttachBridgeSessionOptions {
    pub session_id: String,
    pub api_base_url: String,
    pub ingress_token: SensitiveString,
    /// The worker epoch to run under; the service is asked for one when absent.
    pub epoch: Option<i64>,
    /// The sequence number the read stream resumes after.
    pub initial_sequence_num: Option<i64>,
    pub heartbeat_interval_ms: Option<u64>,
    /// Upload only: the read stream is never opened and inbound messages are not delivered.
    pub outbound_only: bool,
    pub claude_code_version: Option<String>,
    pub callbacks: AttachCallbacks,
    /// The environment the session reads `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_CODE_SESSION_ACCESS_TOKEN`,
    /// `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS` and `CLAUDE_CODE_SIMPLE` from; the process environment when absent.
    pub environment: Option<EnvironmentLookup>,
    /// The process arguments `--bare` is looked for in; the process arguments when absent.
    pub arguments: Option<Vec<String>>,
    /// Rust only; `false` unless set, which is how the upstream entrypoint behaves. When `true` the caller runs the
    /// steps itself ([`BridgeSession::pump`], [`BridgeSession::stream_push`] and the others) instead of the session
    /// starting the heartbeat timer, the upload task and the read stream. The tests that replay one recorded request
    /// at a time are its only users.
    #[doc(hidden)]
    pub caller_driven: bool,
    /// Rust only. The two clocks the liveness check of the read stream compares, which the bundle reads from
    /// `Date.now` and `performance.now`; the system clocks when absent. It lets a test move them the way a paused
    /// or sleeping process does.
    #[doc(hidden)]
    pub stream_clocks: Option<StreamClocks>,
}

impl AttachBridgeSessionOptions {
    /// The options with only the required fields set.
    #[must_use]
    pub fn new(
        session_id: impl Into<String>,
        api_base_url: impl Into<String>,
        ingress_token: impl Into<SensitiveString>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            api_base_url: api_base_url.into(),
            ingress_token: ingress_token.into(),
            epoch: None,
            initial_sequence_num: None,
            heartbeat_interval_ms: None,
            outbound_only: false,
            claude_code_version: None,
            callbacks: AttachCallbacks::default(),
            environment: None,
            arguments: None,
            caller_driven: false,
            stream_clocks: None,
        }
    }
}

/// Why a session could not be attached.
#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    #[error(transparent)]
    InvalidSessionIdError(#[from] InvalidSessionIdError),
    #[error("{}", .0.message())]
    Register(RegisterFailureError),
    #[error("Invalid URL")]
    InvalidUrl,
}

pub(crate) fn lookup_environment(lookup: &Option<EnvironmentLookup>, name: &str) -> Option<String> {
    match lookup {
        Some(lookup) => lookup(name),
        None => SystemEnvironment
            .var_os(OsStr::new(name))
            .and_then(|value| value.into_string().ok()),
    }
}

/// An attached session (`BridgeSessionHandle`).
pub struct BridgeSession<H, P = TokioPacing> {
    pub(crate) http: H,
    pub(crate) pacing: P,
    pub(crate) session_id: String,
    pub(crate) token: SensitiveString,
    pub(crate) api_base_url: String,
    pub(crate) entrypoint: Option<String>,
    pub(crate) session_access_token: Option<SensitiveString>,
    pub(crate) heartbeat_interval_ms: Option<u64>,
    pub(crate) outbound_only: bool,
    pub(crate) worker: Arc<WorkerClient<H, P>>,
    /// Tells the upload task which worker is current after a rebuild.
    pub(crate) worker_tx: watch::Sender<Arc<WorkerClient<H, P>>>,
    pub(crate) lifecycle: Arc<Lifecycle>,
    pub(crate) uploads: Arc<Uploads>,
    pub(crate) upload_task: Option<JoinHandle<()>>,
    pub(crate) stream_task: Option<JoinHandle<()>>,
    /// What the read stream, the host and the tasks share.
    pub(crate) shared: Arc<Runtime<H, P>>,
    pub(crate) scheduled: bool,
    pub(crate) stream_clocks: Option<StreamClocks>,
}

/// `attachBridgeSession`: registers the worker when no epoch is given, and starts it (`connect`).
///
/// A `reqwest::Client` reports a registration that never got an answer with the upstream error codes only when it
/// resolves host names with [`crate::adapters::GetAddrInfoResolver`] (installed through
/// `reqwest::ClientBuilder::dns_resolver`) or with a resolver that reports its failures as
/// [`crate::adapters::NameLookupError`]; otherwise a host that cannot be found is reported as `ERR_NETWORK` instead of
/// `ENOTFOUND`.
pub async fn attach_bridge_session<H, P>(
    http: H,
    pacing: P,
    options: AttachBridgeSessionOptions,
) -> Result<BridgeSession<H, P>, AttachError>
where
    H: HttpExecute + Clone + Send + Sync + 'static,
    P: WorkerPacing + Clone + Send + Sync + 'static,
{
    let AttachBridgeSessionOptions {
        session_id,
        api_base_url,
        ingress_token,
        epoch,
        initial_sequence_num,
        heartbeat_interval_ms,
        outbound_only,
        claude_code_version,
        mut callbacks,
        environment,
        arguments,
        caller_driven,
        stream_clocks,
    } = options;
    let entrypoint = lookup_environment(&environment, "CLAUDE_CODE_ENTRYPOINT");
    let session_access_token =
        lookup_environment(&environment, "CLAUDE_CODE_SESSION_ACCESS_TOKEN").map(SensitiveString::from);
    let worker = Arc::new(WorkerClient::new(
        http.clone(),
        pacing.clone(),
        WorkerConfig {
            api_base_url: api_base_url.clone(),
            session_id: session_id.clone(),
            token: ingress_token.clone(),
            entrypoint: entrypoint.clone(),
            heartbeat_interval_ms,
            session_access_token: session_access_token.clone(),
            non_origin_403: crate::bridge_worker::NonOrigin403::Counted,
        },
    )?);
    let epoch = match epoch {
        Some(epoch) => epoch,
        None => worker.register_worker().await.map_err(AttachError::Register)?,
    };
    // The upstream entrypoint builds the session URL after the epoch is known and throws when it does not parse.
    url::Url::parse(worker.session_url()).map_err(|_| AttachError::InvalidUrl)?;
    callbacks.control.has_stop_task = callbacks.asynchronous.on_stop_task.is_some();
    callbacks.control.has_background_tasks = callbacks.asynchronous.on_background_tasks.is_some();
    callbacks.control.has_side_question = callbacks.asynchronous.on_side_question.is_some();
    let lifecycle = Arc::new(Lifecycle::new(callbacks.on_close.take()));
    let uploads = Arc::new(Uploads::default());
    let arguments = arguments.unwrap_or_else(|| std::env::args().collect());
    let switch_environment = environment.clone();
    let async_control = AsyncControl::new(
        &session_id,
        {
            let (lifecycle, uploads) = (Arc::clone(&lifecycle), Arc::clone(&uploads));
            Box::new(move |frame| {
                if !lifecycle.is_closed() {
                    uploads.push(frame);
                }
            })
        },
        Box::new(|| uuid::Uuid::new_v4().to_string()),
        callbacks.asynchronous,
        Box::new(move || background_tasks_disabled(|name| lookup_environment(&switch_environment, name), &arguments)),
    );
    let (worker_tx, worker_rx) = watch::channel(Arc::clone(&worker));
    let shared = Arc::new(Runtime::new(
        Core {
            router: InboundRouter::default(),
            pending_prompts: Vec::new(),
            on_inbound_message: callbacks.on_inbound_message,
            on_permission_response: callbacks.on_permission_response,
            control: callbacks.control,
            async_control,
            tasks: Vec::new(),
        },
        StreamSession::seeded(initial_sequence_num),
        Arc::clone(&lifecycle),
        Arc::clone(&uploads),
        worker_rx,
        session_id.clone(),
        claude_code_version,
        outbound_only,
        !caller_driven,
    ));
    let mut session = BridgeSession {
        http,
        pacing,
        session_id,
        token: ingress_token,
        api_base_url,
        entrypoint,
        session_access_token,
        heartbeat_interval_ms,
        outbound_only,
        worker_tx,
        worker,
        lifecycle,
        uploads,
        upload_task: None,
        stream_task: None,
        shared,
        scheduled: !caller_driven,
        stream_clocks,
    };
    session.connect(epoch).await;
    session.start_runtime();
    Ok(session)
}

impl<H, P> BridgeSession<H, P>
where
    H: HttpExecute + Send + Sync + 'static,
    P: WorkerPacing + Send + Sync + 'static,
{
    /// `sessionId`.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// `getSequenceNum`: the highest sequence number of a durable event the read stream saw.
    #[must_use]
    pub fn sequence_num(&self) -> i64 {
        self.shared.stream().last_sequence()
    }

    /// `getEpoch`.
    #[must_use]
    pub fn epoch(&self) -> i64 {
        self.worker.epoch()
    }

    /// `isConnected`: the worker is registered and the session is neither closed nor ended.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.lifecycle.is_connected()
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.lifecycle.is_closed()
    }

    /// Registers the worker as idle. A failure the service refused for good closes the session with `4091`, a
    /// terminal request-path condition with `4090`.
    pub(crate) async fn connect(&mut self, epoch: i64) {
        match self.worker.initialize(epoch).await {
            Ok(()) => self.lifecycle.set_connected(true),
            Err(InitError::Failed(_)) => self.end(CLOSE_INIT_FAILED),
            Err(InitError::Mismatch(reason)) => self.end(reason.close_code(false)),
        }
    }

    /// The transport closing itself: the host hears about it once, unless it closed the handle first.
    pub(crate) fn end(&self, code: u16) {
        self.shared.end(code);
    }

    /// Ends the session when a request hit a terminal condition.
    pub(crate) fn settle<T>(&mut self, result: Result<T, MismatchReason>) -> Option<T> {
        self.shared.settle(result)
    }

    /// The request to open the read stream with. `None` for an upload-only or a finished session.
    #[must_use]
    pub fn stream_request(&self) -> Option<StreamRequest> {
        (!self.outbound_only && !self.lifecycle.is_ended() && !self.is_closed()).then(|| self.shared.stream().request())
    }

    /// The answer to the request for the read stream. A status that closes the session ends it with the status as
    /// the close code.
    pub fn stream_opened(&mut self, status: u16) -> StreamOpen {
        let open = self.shared.stream().opened(status);
        if let StreamOpen::Close(code) = open {
            self.end(code);
        }
        open
    }

    /// The read stream was lost; how long to wait before asking again.
    pub fn stream_lost(&mut self, random: f64) -> Duration {
        self.shared.stream().lost(random)
    }

    /// The read stream went silent; see [`StreamSession::liveness_expired`].
    pub fn stream_silent(&mut self, wall_silent: Duration, monotonic_silent: Duration, random: f64) -> Reopen {
        self.shared
            .stream()
            .liveness_expired(wall_silent, monotonic_silent, random)
    }

    /// Hands the bytes read from the stream to the session: every event is acknowledged as received and processed,
    /// a heartbeat probe beats at once, and the lines the stream carries go through [`BridgeSession::deliver_line`].
    pub async fn stream_push(&mut self, bytes: &[u8]) {
        let effects = self.shared.stream().push(bytes);
        for effect in effects {
            self.shared.apply(effect).await;
        }
    }

    /// One heartbeat; a terminal answer ends the session.
    pub async fn heartbeat(&mut self) -> bool {
        self.shared.beat().await
    }

    /// What the inbound reader does with one line (`onData`): a control response settles its pending prompt, a
    /// control request is answered, a cancel aborts the work it names and a user message goes to the host.
    pub async fn deliver_line(&mut self, line: &str) {
        self.shared.deliver_line(line).await;
    }

    pub(crate) fn queue_frame(&self, frame: Value) {
        self.shared.queue_frame(frame);
    }

    /// Moves the frames the control dispatch wrote into the upload queue.
    pub(crate) async fn drain_outbox(&mut self) {
        self.shared.drain_outbox().await;
    }

    /// Waits for the host calls the control dispatch started and queues their replies.
    pub async fn settle_control(&mut self) {
        self.shared.settle_control().await;
    }

    /// Sends what is queued: the state and metadata reports, the events and the acknowledgements.
    pub async fn pump(&mut self) {
        self.pump_attempts(None).await;
    }

    /// [`BridgeSession::pump`] that makes at most `attempts` requests for each queue, so a failed upload leaves its
    /// retry wait for the caller to see; see [`WorkerClient::pump_events`].
    pub async fn pump_attempts(&mut self, attempts: Option<u32>) {
        if self.lifecycle.is_ended() || self.is_closed() {
            return;
        }
        self.drain_outbox().await;
        let reports = self.worker.pump(attempts).await;
        self.settle(reports);
        let events = self.worker.pump_events(attempts).await;
        self.settle(events);
        let deliveries = self.worker.pump_deliveries(attempts).await;
        self.settle(deliveries);
    }

    /// Marks the session closed and aborts the host calls in flight; false when it already was.
    pub(crate) fn mark_closed(&mut self) -> bool {
        if !self.lifecycle.mark_closed() {
            return false;
        }
        self.shared.core().async_control.cancel_all();
        if let Some(task) = self.stream_task.take() {
            task.abort();
        }
        true
    }

    /// `close`: the host leaves. Open requests are aborted, the worker says goodbye and what was queued is dropped.
    pub async fn close(&mut self) {
        if !self.mark_closed() {
            return;
        }
        // The post the uploader had started goes out first, then the rows that waited for a window, each in its own
        // post, the way the upstream uploader sends what is queued before it says goodbye. Rows queued behind the
        // started post are dropped.
        let ready = self.uploads.take_started();
        let waiting = self.uploads.close_window();
        if !self.lifecycle.is_ended() {
            for rows in [ready, waiting] {
                if !rows.is_empty() {
                    let _held = self.worker.submit_events(rows).await;
                    let _sent = self.worker.pump_events(Some(1)).await;
                }
            }
            self.worker.goodbye().await;
        }
        wind_down(self.worker.as_ref());
        self.lifecycle.set_connected(false);
        self.uploads.wake();
    }
}

impl<H, P> BridgeSession<H, P>
where
    H: HttpExecute + Send + Sync + 'static,
    P: WorkerPacing + Send + Sync + 'static,
{
    /// Starts what the upstream entrypoint starts once the worker is registered (`startHeartbeat`, the upload
    /// flush and `openStream`): the heartbeat timer, the upload task and the read stream, unless the caller drives
    /// the steps itself or the session already ended. A rebuilt transport starts them again for its new worker.
    pub(crate) fn start_runtime(&mut self) {
        if !self.scheduled || self.lifecycle.is_ended() || self.is_closed() {
            return;
        }
        let weak = Arc::downgrade(&self.shared);
        self.shared.set_heartbeat(Some(HeartbeatDriver::start(
            Arc::clone(&self.worker),
            self.lifecycle.closed_flag(),
            move |reason| {
                if let Some(shared) = weak.upgrade() {
                    shared.end(reason.close_code(false));
                }
            },
        )));
        self.spawn_uploads();
        if let Some(task) = self.stream_task.take() {
            task.abort();
        }
        if !self.outbound_only {
            self.stream_task = Some(tokio::spawn(read_stream(
                Arc::clone(&self.shared),
                Arc::clone(&self.worker),
                self.stream_clocks.clone().unwrap_or_default(),
            )));
        }
    }

    /// Starts the task that uploads in the background, the way the upstream handle does: every write wakes it, it
    /// moves the frames into the worker and sends what is queued until the service has taken it, and it ends the
    /// session when the service refuses for good. Starting it twice does nothing. [`BridgeSession::pump`] stays
    /// available and shares the queues with it.
    pub fn spawn_uploads(&mut self) {
        if self.upload_task.is_some() {
            return;
        }
        self.upload_task = Some(spawn_uploads(
            Arc::clone(&self.uploads),
            Arc::clone(&self.lifecycle),
            self.worker_tx.subscribe(),
        ));
    }

    /// Waits until the upload task has handled everything written so far.
    pub async fn uploaded(&self) {
        self.uploads.settled().await;
    }

    /// Waits for the upload task to return, which it does once the host has closed the session.
    pub async fn join_uploads(&mut self) {
        if let Some(task) = self.upload_task.take() {
            let _ = task.await;
        }
    }
}

/// The read stream of one worker: the loop that opens and reads it, and the loop that applies what it carries, one
/// effect after the other. When the service refuses the stream for good the session ends with the status.
async fn read_stream<H, P>(shared: Arc<Runtime<H, P>>, worker: Arc<WorkerClient<H, P>>, clocks: StreamClocks)
where
    H: HttpExecute + Send + Sync + 'static,
    P: WorkerPacing + Send + Sync + 'static,
{
    let connector = HttpStreamConnector::new(Arc::clone(&worker));
    let (send, mut arrived) = tokio::sync::mpsc::unbounded_channel();
    let reading = async {
        let code = run_stream(
            &connector,
            shared.stream_cell(),
            &clocks,
            &mut || worker.pacing().jitter(),
            &mut |effect| {
                let _ = send.send(effect);
            },
        )
        .await;
        drop(send);
        code
    };
    let applying = async {
        while let Some(effect) = arrived.recv().await {
            shared.apply(effect).await;
        }
    };
    let (code, ()) = tokio::join!(reading, applying);
    shared.end(code);
}

impl<H, P> Drop for BridgeSession<H, P> {
    fn drop(&mut self) {
        if let Some(task) = self.stream_task.take() {
            task.abort();
        }
        if let Some(task) = self.upload_task.take() {
            task.abort();
        }
    }
}
