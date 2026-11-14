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

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::SensitiveString;
use crate::adapters::HttpExecute;
use crate::bridge_session_runtime::{Lifecycle, Uploads, spawn_uploads, wind_down};
use crate::bridge_worker::{InvalidSessionId, TokioPacing, WorkerClient, WorkerConfig, WorkerPacing};
use crate::bridge_worker_control::{ControlCallbacks, ControlReply, ControlSession, answer_control_request};
use crate::bridge_worker_control_async::{AsyncControl, AsyncControlCallbacks, background_tasks_disabled};
use crate::bridge_worker_delivery::DeliveryStatus;
use crate::bridge_worker_events::ClientEvent;
use crate::bridge_worker_inbound::{Inbound, InboundRouter};
use crate::bridge_worker_stream::{Reopen, StreamEffect, StreamOpen, StreamRequest, StreamSession};
use crate::bridge_worker_types::{InitError, MismatchReason, RegisterFailure};

/// `onClose` when the worker could not be initialised for good (`4091`).
pub const CLOSE_INIT_FAILED: u16 = 4091;

/// What the host reads the environment with (`process.env`).
pub type EnvironmentLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// `onInboundMessage`.
pub type InboundMessageCallback = Box<dyn FnMut(Value) + Send>;
/// `onPermissionResponse`: `Some(false)` keeps the pending prompt, anything else settles it (the upstream test is
/// `!== false`).
pub type PermissionResponseCallback = Box<dyn FnMut(&Value) -> Option<bool> + Send>;
/// `onClose`.
pub type CloseCallback = Box<dyn FnMut(u16) + Send>;

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
        }
    }
}

/// Why a session could not be attached.
#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    #[error(transparent)]
    InvalidSessionId(#[from] InvalidSessionId),
    #[error("{}", .0.message())]
    Register(RegisterFailure),
    #[error("Invalid URL")]
    InvalidUrl,
}

pub(crate) fn lookup_environment(lookup: &Option<EnvironmentLookup>, name: &str) -> Option<String> {
    match lookup {
        Some(lookup) => lookup(name),
        None => std::env::var(name).ok(),
    }
}

/// The key a request id has in the table of pending prompts: ids are strings or numbers, which print distinctly.
pub(crate) fn request_key(id: Option<&Value>) -> String {
    id.map_or_else(|| "undefined".to_owned(), Value::to_string)
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
    pub(crate) claude_code_version: Option<String>,
    pub(crate) worker: Arc<WorkerClient<H, P>>,
    /// Tells the upload task which worker is current after a rebuild.
    pub(crate) worker_tx: watch::Sender<Arc<WorkerClient<H, P>>>,
    pub(crate) stream: StreamSession,
    pub(crate) router: InboundRouter,
    pub(crate) lifecycle: Arc<Lifecycle>,
    pub(crate) pending_prompts: Vec<(String, Value)>,
    pub(crate) uploads: Arc<Uploads>,
    pub(crate) upload_task: Option<JoinHandle<()>>,
    pub(crate) tasks: Vec<JoinHandle<()>>,
    on_inbound_message: Option<InboundMessageCallback>,
    on_permission_response: Option<PermissionResponseCallback>,
    control: ControlCallbacks,
    async_control: AsyncControl,
}

/// `attachBridgeSession`: registers the worker when no epoch is given, and starts it (`connect`).
pub async fn attach_bridge_session<H, P>(
    http: H,
    pacing: P,
    options: AttachBridgeSessionOptions,
) -> Result<BridgeSession<H, P>, AttachError>
where
    H: HttpExecute + Clone,
    P: WorkerPacing + Clone,
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
        claude_code_version,
        worker_tx: watch::channel(Arc::clone(&worker)).0,
        worker,
        stream: StreamSession::seeded(initial_sequence_num),
        router: InboundRouter::default(),
        lifecycle,
        pending_prompts: Vec::new(),
        uploads,
        upload_task: None,
        tasks: Vec::new(),
        on_inbound_message: callbacks.on_inbound_message,
        on_permission_response: callbacks.on_permission_response,
        control: callbacks.control,
        async_control,
    };
    session.connect(epoch).await;
    Ok(session)
}

impl<H: HttpExecute, P: WorkerPacing> BridgeSession<H, P> {
    /// `sessionId`.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// `getSequenceNum`: the highest sequence number of a durable event the read stream saw.
    #[must_use]
    pub fn sequence_num(&self) -> i64 {
        self.stream.last_sequence()
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
        self.lifecycle.end(code, || wind_down(self.worker.as_ref()));
    }

    /// Ends the session when a request hit a terminal condition.
    pub(crate) fn settle<T>(&mut self, result: Result<T, MismatchReason>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(reason) => {
                self.end(reason.close_code(false));
                None
            }
        }
    }

    /// The request to open the read stream with. `None` for an upload-only or a finished session.
    #[must_use]
    pub fn stream_request(&self) -> Option<StreamRequest> {
        (!self.outbound_only && !self.lifecycle.is_ended() && !self.is_closed()).then(|| self.stream.request())
    }

    /// The answer to the request for the read stream. A status that closes the session ends it with the status as
    /// the close code.
    pub fn stream_opened(&mut self, status: u16) -> StreamOpen {
        let open = self.stream.opened(status);
        if let StreamOpen::Close(code) = open {
            self.end(code);
        }
        open
    }

    /// The read stream was lost; how long to wait before asking again.
    pub fn stream_lost(&mut self, random: f64) -> Duration {
        self.stream.lost(random)
    }

    /// The read stream went silent; see [`StreamSession::liveness_expired`].
    pub fn stream_silent(&mut self, wall_silent: Duration, monotonic_silent: Duration, random: f64) -> Reopen {
        self.stream.liveness_expired(wall_silent, monotonic_silent, random)
    }

    /// Hands the bytes read from the stream to the session: every event is acknowledged as received and processed,
    /// a heartbeat probe beats at once, and the lines the stream carries go through [`BridgeSession::deliver_line`].
    pub async fn stream_push(&mut self, bytes: &[u8]) {
        for effect in self.stream.push(bytes) {
            match effect {
                StreamEffect::Deliver(line) => self.deliver_line(&line).await,
                StreamEffect::Acknowledge { event_id } => {
                    let received = self.worker.report_delivery(&event_id, DeliveryStatus::Received).await;
                    self.settle(received);
                    let processed = self.worker.report_delivery(&event_id, DeliveryStatus::Processed).await;
                    self.settle(processed);
                }
                StreamEffect::HeartbeatProbe => {
                    self.heartbeat().await;
                }
            }
        }
    }

    /// One heartbeat; a terminal answer ends the session.
    pub async fn heartbeat(&mut self) -> bool {
        if self.lifecycle.is_ended() || self.is_closed() {
            return false;
        }
        let outcome = self.worker.heartbeat().await;
        self.settle(outcome).is_some()
    }

    /// What the inbound reader does with one line (`onData`): a control response settles its pending prompt, a
    /// control request is answered, a cancel aborts the work it names and a user message goes to the host.
    pub async fn deliver_line(&mut self, line: &str) {
        let Some(inbound) = self.router.route(line) else {
            return;
        };
        match inbound {
            Inbound::ControlResponse(message) => self.permission_response(&message),
            Inbound::ControlRequest(request) => self.answer(&request),
            Inbound::ControlCancel(request_id) => self.async_control.cancel(&Value::String(request_id)),
            Inbound::QueuedNotification(_) => {}
            Inbound::User(message) => {
                if !self.outbound_only
                    && let Some(callback) = self.on_inbound_message.as_mut()
                {
                    callback(message);
                }
            }
        }
        self.drain_outbox().await;
    }

    fn permission_response(&mut self, message: &Value) {
        let keep = self
            .on_permission_response
            .as_mut()
            .and_then(|callback| callback(message))
            == Some(false);
        let response = message.get("response");
        let key = request_key(response.and_then(|response| response.get("request_id")));
        let dialog_refused = response
            .and_then(|response| response.get("subtype"))
            .is_some_and(|subtype| subtype == "error")
            && self
                .pending_prompts
                .iter()
                .find(|(pending, _)| *pending == key)
                .is_some_and(|(_, request)| request["request"]["subtype"] == "request_user_dialog");
        if !keep && !dialog_refused {
            self.pending_prompts.retain(|(pending, _)| *pending != key);
        }
    }

    fn answer(&mut self, request: &Value) {
        let prompts: Vec<Value> = self
            .pending_prompts
            .iter()
            .map(|(_, request)| request.clone())
            .collect();
        let reply = answer_control_request(
            request,
            &ControlSession {
                session_id: &self.session_id,
                claude_code_version: self.claude_code_version.as_deref(),
                pid: std::process::id(),
                pending_prompts: &prompts,
            },
            &mut self.control,
        );
        let task = match reply {
            ControlReply::Frame(frame) => {
                self.queue_frame(frame);
                None
            }
            ControlReply::Deferred(work) => self.async_control.start(request, work),
            ControlReply::Pending(pending) => Some(self.async_control.finish_pending(request, pending)),
        };
        if let Some(task) = task {
            self.tasks.push(tokio::spawn(task));
        }
    }

    pub(crate) fn queue_frame(&self, frame: Value) {
        if !self.is_closed() {
            self.uploads.push(frame);
        }
    }

    /// Moves the frames the control dispatch wrote into the upload queue.
    pub(crate) async fn drain_outbox(&mut self) {
        let frames = self.uploads.take();
        if frames.is_empty() || self.is_closed() {
            return;
        }
        let events: Vec<ClientEvent> = frames.into_iter().filter_map(client_event).collect();
        let submitted = self.worker.submit_events(events).await;
        self.settle(submitted);
    }

    /// Waits for the host calls the control dispatch started and queues their replies.
    pub async fn settle_control(&mut self) {
        for task in std::mem::take(&mut self.tasks) {
            let _ = task.await;
        }
        self.drain_outbox().await;
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
        self.async_control.cancel_all();
        true
    }

    /// `close`: the host leaves. Open requests are aborted, the worker says goodbye and what was queued is dropped.
    pub async fn close(&mut self) {
        if !self.mark_closed() {
            return;
        }
        if !self.lifecycle.is_ended() {
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

impl<H, P> Drop for BridgeSession<H, P> {
    fn drop(&mut self) {
        if let Some(task) = self.upload_task.take() {
            task.abort();
        }
    }
}

/// `toClientEvent`: the row as it is uploaded. Thinking-token rows and `live_only` control responses are live-view
/// rows. A live-only control response that would not fit is replaced by an error that says so (`rF`).
pub(crate) fn client_event(frame: Value) -> Option<ClientEvent> {
    let Value::Object(mut payload) = frame else {
        return None;
    };
    let live_only = payload.get("type").is_some_and(|kind| kind == "control_response")
        && payload.get("live_only").is_some_and(|flag| flag == true);
    if live_only && serde_json::to_string(&payload).map_or(0, |text| text.len()) > LIVE_ONLY_LIMIT {
        payload = oversized_reply(&payload);
    }
    let thinking = payload.get("type").is_some_and(|kind| kind == "system")
        && payload
            .get("subtype")
            .is_some_and(|subtype| subtype == "thinking_tokens");
    let historical = payload.get("historical").is_some_and(|flag| flag == true);
    Some(ClientEvent {
        payload,
        historical,
        ephemeral: live_only || thinking,
    })
}

/// `$y`: how large a live-only reply may be, in bytes.
const LIVE_ONLY_LIMIT: usize = 57_344;
const OVERSIZED_REPLY: &str = "The reply is too large to be sent without storing it";

fn oversized_reply(payload: &Map<String, Value>) -> Map<String, Value> {
    let mut replaced = Map::new();
    replaced.insert("type".to_owned(), Value::String("control_response".to_owned()));
    replaced.insert("live_only".to_owned(), Value::Bool(true));
    if let Some(uuid) = payload.get("uuid").filter(|uuid| uuid.is_string()) {
        replaced.insert("uuid".to_owned(), uuid.clone());
    }
    let mut response = Map::new();
    response.insert("subtype".to_owned(), Value::String("error".to_owned()));
    if let Some(id) = payload.get("response").and_then(|response| response.get("request_id")) {
        response.insert("request_id".to_owned(), id.clone());
    }
    response.insert("error".to_owned(), Value::String(OVERSIZED_REPLY.to_owned()));
    replaced.insert("response".to_owned(), Value::Object(response));
    replaced
}
