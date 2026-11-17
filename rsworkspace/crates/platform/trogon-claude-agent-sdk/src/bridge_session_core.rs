//! What the read stream and the host both reach in an attached session: the inbound router, the table of pending
//! prompts, the host callbacks and the stream state. The upstream handle is one object that its timers and its callers
//! share; here the shared part is [`Runtime`], held in an `Arc` by the session and by the tasks it starts, so a
//! frame that arrives is handled the same way whether the caller feeds it in or the stream task does.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::Value;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::adapters::HttpExecute;
use crate::bridge_session_callbacks::{InboundMessageCallback, PermissionResponseCallback, request_key};
use crate::bridge_session_runtime::{Lifecycle, Uploads, wind_down};
use crate::bridge_worker::{WorkerClient, WorkerPacing};
use crate::bridge_worker_control::{BridgeControlReply, ControlCallbacks, ControlSession, answer_control_request};
use crate::bridge_worker_control_async::AsyncControl;
use crate::bridge_worker_delivery::DeliveryStatus;
use crate::bridge_worker_heartbeat::HeartbeatDriver;
use crate::bridge_worker_inbound::{Inbound, InboundRouter};
use crate::bridge_worker_stream::{StreamEffect, StreamSession};
use crate::bridge_worker_types::MismatchReason;

/// The state the inbound lines change.
pub(crate) struct Core {
    pub(crate) router: InboundRouter,
    pub(crate) pending_prompts: Vec<(String, Value)>,
    pub(crate) on_inbound_message: Option<InboundMessageCallback>,
    pub(crate) on_permission_response: Option<PermissionResponseCallback>,
    pub(crate) control: ControlCallbacks,
    pub(crate) async_control: AsyncControl,
    pub(crate) tasks: Vec<JoinHandle<()>>,
}

/// The part of the session the tasks it starts share with it. No lock here is held across an await.
pub(crate) struct Runtime<H, P> {
    core: Mutex<Core>,
    stream: Mutex<StreamSession>,
    heartbeat: Mutex<Option<HeartbeatDriver<H, P>>>,
    pub(crate) lifecycle: Arc<Lifecycle>,
    pub(crate) uploads: Arc<Uploads>,
    worker: watch::Receiver<Arc<WorkerClient<H, P>>>,
    session_id: String,
    claude_code_version: Option<String>,
    outbound_only: bool,
    /// The upload task drains the frames, so a line does not.
    scheduled: bool,
}

impl<H, P> Runtime<H, P> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        core: Core,
        stream: StreamSession,
        lifecycle: Arc<Lifecycle>,
        uploads: Arc<Uploads>,
        worker: watch::Receiver<Arc<WorkerClient<H, P>>>,
        session_id: String,
        claude_code_version: Option<String>,
        outbound_only: bool,
        scheduled: bool,
    ) -> Self {
        Self {
            core: Mutex::new(core),
            stream: Mutex::new(stream),
            heartbeat: Mutex::new(None),
            lifecycle,
            uploads,
            worker,
            session_id,
            claude_code_version,
            outbound_only,
            scheduled,
        }
    }

    pub(crate) fn core(&self) -> MutexGuard<'_, Core> {
        self.core.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn stream(&self) -> MutexGuard<'_, StreamSession> {
        self.stream.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The stream state behind its lock, for the loop that reads the stream.
    pub(crate) fn stream_cell(&self) -> &Mutex<StreamSession> {
        &self.stream
    }

    /// Replaces the heartbeat timer; the old one stops when it is dropped.
    pub(crate) fn set_heartbeat(&self, driver: Option<HeartbeatDriver<H, P>>) {
        *self.heartbeat.lock().unwrap_or_else(PoisonError::into_inner) = driver;
    }

    pub(crate) fn queue_frame(&self, frame: Value) {
        if !self.lifecycle.is_closed() {
            self.uploads.push(frame);
        }
    }

    fn route_inbound(&self, inbound: Inbound, core: &mut Core) {
        match inbound {
            Inbound::ControlResponse(message) => self.permission_response(core, &message),
            Inbound::ControlRequest(request) => self.answer(core, &request),
            Inbound::ControlCancel(request_id) => core.async_control.cancel(&Value::String(request_id)),
            Inbound::QueuedNotification(_) => {}
            Inbound::User(message) => {
                if !self.outbound_only
                    && let Some(callback) = core.on_inbound_message.as_mut()
                {
                    callback(message);
                }
            }
        }
    }

    fn permission_response(&self, core: &mut Core, message: &Value) {
        let keep = core
            .on_permission_response
            .as_mut()
            .and_then(|callback| callback(message))
            == Some(false);
        let response = message.get("response");
        let key = request_key(response.and_then(|response| response.get("request_id")));
        let dialog_refused = response
            .and_then(|response| response.get("subtype"))
            .is_some_and(|subtype| subtype == "error")
            && core
                .pending_prompts
                .iter()
                .find(|(pending, _)| *pending == key)
                .is_some_and(|(_, request)| request["request"]["subtype"] == "request_user_dialog");
        if !keep && !dialog_refused {
            core.pending_prompts.retain(|(pending, _)| *pending != key);
        }
    }

    fn answer(&self, core: &mut Core, request: &Value) {
        let prompts: Vec<Value> = core
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
            &mut core.control,
        );
        let task = match reply {
            BridgeControlReply::Frame(frame) => {
                self.queue_frame(frame);
                None
            }
            BridgeControlReply::Deferred(work) => core.async_control.start(request, work),
            BridgeControlReply::Pending(pending) => Some(core.async_control.finish_pending(request, pending)),
        };
        if let Some(task) = task {
            core.tasks.push(tokio::spawn(task));
        }
    }
}

impl<H: HttpExecute + Send + Sync + 'static, P: WorkerPacing + Send + Sync + 'static> Runtime<H, P> {
    pub(crate) fn worker(&self) -> Arc<WorkerClient<H, P>> {
        Arc::clone(&self.worker.borrow())
    }

    /// The transport closing itself: the host hears about it once, unless it closed the handle first.
    pub(crate) fn end(&self, code: u16) {
        self.lifecycle.end(code, || wind_down(self.worker().as_ref()));
    }

    /// Ends the session when a request hit a terminal condition.
    pub(crate) fn settle<T>(&self, result: Result<T, MismatchReason>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(reason) => {
                self.end(reason.close_code(false));
                None
            }
        }
    }

    /// Moves the frames the control dispatch wrote into the upload queue.
    pub(crate) async fn drain_outbox(&self) {
        self.uploads.fire();
        let events = self.uploads.take();
        if events.is_empty() || self.lifecycle.is_closed() {
            return;
        }
        let submitted = self.worker().submit_events(events).await;
        self.settle(submitted);
    }

    /// What the inbound reader does with one line (`onData`).
    pub(crate) async fn deliver_line(&self, line: &str) {
        {
            let mut core = self.core();
            let Some(inbound) = core.router.route(line) else {
                return;
            };
            self.route_inbound(inbound, &mut core);
        }
        if !self.scheduled {
            self.drain_outbox().await;
        }
    }

    /// One effect of the frames the stream carried.
    pub(crate) async fn apply(&self, effect: StreamEffect) {
        match effect {
            StreamEffect::Deliver(line) => self.deliver_line(&line).await,
            StreamEffect::Acknowledge { event_id } => {
                let received = self.worker().report_delivery(&event_id, DeliveryStatus::Received).await;
                self.settle(received);
                let processed = self
                    .worker()
                    .report_delivery(&event_id, DeliveryStatus::Processed)
                    .await;
                self.settle(processed);
            }
            StreamEffect::HeartbeatProbe => {
                if !self.probe() {
                    self.beat().await;
                }
            }
        }
    }

    /// Asks the heartbeat timer for a beat; false when there is no timer.
    fn probe(&self) -> bool {
        match &*self.heartbeat.lock().unwrap_or_else(PoisonError::into_inner) {
            Some(driver) => {
                driver.probe();
                true
            }
            None => false,
        }
    }

    /// One heartbeat; a terminal answer ends the session.
    pub(crate) async fn beat(&self) -> bool {
        if self.lifecycle.is_ended() || self.lifecycle.is_closed() {
            return false;
        }
        let outcome = self.worker().heartbeat().await;
        self.settle(outcome).is_some()
    }

    /// Waits for the host calls the control dispatch started and queues their replies.
    pub(crate) async fn settle_control(&self) {
        let tasks = std::mem::take(&mut self.core().tasks);
        for task in tasks {
            let _ = task.await;
        }
        self.drain_outbox().await;
    }
}
