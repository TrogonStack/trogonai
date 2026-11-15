//! `BridgeSessionHandle` (offset 1198000): what the host calls on an attached session. The writes queue behind the
//! control replies the session itself produced and go out with [`BridgeSession::pump`]; the upstream calls return
//! at once and upload in the background, which is the same thing seen from the caller.
//!

use std::sync::Arc;

use serde_json::{Map, Value};

use crate::SensitiveString;
use crate::adapters::HttpExecute;
use crate::bridge_session::BridgeSession;
use crate::bridge_session_callbacks::request_key;
use crate::bridge_session_runtime::wind_down;
use crate::bridge_types::SessionState;
use crate::bridge_worker::{
    InvalidSessionIdError, NonOrigin403, WorkerClient, WorkerConfig, WorkerPacing, session_url,
};
use crate::bridge_worker_delivery::DeliveryStatus;
use crate::bridge_worker_events::ClientEvent;
use crate::bridge_worker_stream::StreamSession;

mod constants;

use constants::NOT_CARRIED_AS_ECHO;

/// What `reconnectTransport` is given.
pub struct ReconnectOptions {
    pub api_base_url: String,
    pub ingress_token: SensitiveString,
    /// The epoch to run under; the one the session had when absent.
    pub epoch: Option<i64>,
}

/// Why `reconnectTransport` rejected.
#[derive(Debug, thiserror::Error)]
pub enum ReconnectError {
    #[error(transparent)]
    InvalidSessionIdError(#[from] InvalidSessionIdError),
    /// `new URL` threw on the session URL built from the given base. The old worker's undelivered events are
    /// dropped and the session is closed without telling the host.
    #[error("Invalid URL")]
    InvalidUrl,
}

/// A JSON object with its keys in the order given. These frames carry plain scalars and nested objects, so
/// building them by hand cannot fail the way serializing a typed payload can.
fn object<const N: usize>(entries: [(&str, Value); N]) -> Value {
    Value::Object(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

fn text(value: &str) -> Value {
    Value::String(value.to_owned())
}

fn zero() -> Value {
    Value::from(0_u64)
}

/// `Jp`: the usage block of a result nobody ran a turn for.
fn zero_usage() -> Value {
    object([
        ("output_tokens_details", object([("thinking_tokens", zero())])),
        ("input_tokens", zero()),
        ("cache_creation_input_tokens", zero()),
        ("cache_read_input_tokens", zero()),
        ("output_tokens", zero()),
        (
            "server_tool_use",
            object([("web_search_requests", zero()), ("web_fetch_requests", zero())]),
        ),
        ("service_tier", text("standard")),
        (
            "cache_creation",
            object([
                ("ephemeral_1h_input_tokens", zero()),
                ("ephemeral_5m_input_tokens", zero()),
            ]),
        ),
        ("inference_geo", text("")),
        ("iterations", Value::Array(Vec::new())),
        ("speed", text("standard")),
        ("fallback_credit", Value::Null),
    ])
}

impl<H, P> BridgeSession<H, P>
where
    H: HttpExecute + Send + Sync + 'static,
    P: WorkerPacing + Send + Sync + 'static,
{
    fn with_session_id(&self, mut frame: Map<String, Value>) -> Value {
        frame.insert("session_id".to_owned(), Value::String(self.session_id.clone()));
        Value::Object(frame)
    }

    /// `write`: a frame whose `uuid` the session already wrote or received is dropped; the others go out with the
    /// session id.
    pub fn write(&mut self, frame: Map<String, Value>) {
        if self.is_closed() {
            return;
        }
        if let Some(uuid) = frame
            .get("uuid")
            .and_then(Value::as_str)
            .filter(|uuid| !uuid.is_empty())
            && !self.shared.core().router.remember_echo(uuid)
        {
            return;
        }
        let frame = self.with_session_id(frame);
        self.queue_frame(frame);
    }

    /// `sendResult`: the empty success result that ends a turn nobody ran.
    pub fn send_result(&mut self) {
        if self.is_closed() {
            return;
        }
        let frame = object([
            ("type", text("result")),
            ("duration_ms", zero()),
            ("duration_api_ms", zero()),
            ("total_cost_usd", zero()),
            ("usage", zero_usage()),
            ("modelUsage", object([])),
            ("permission_denials", Value::Array(Vec::new())),
            ("session_id", Value::String(self.session_id.clone())),
            ("uuid", Value::String(uuid::Uuid::new_v4().to_string())),
            ("subtype", text("success")),
            ("is_error", Value::Bool(false)),
            ("num_turns", zero()),
            ("result", text("")),
            ("stop_reason", Value::Null),
        ]);
        self.queue_frame(frame);
    }

    /// `sendControlRequest`: a permission or dialog prompt waits in the table of pending prompts until it is
    /// answered or cancelled.
    pub fn send_control_request(&mut self, request: Map<String, Value>) {
        if self.is_closed() {
            return;
        }
        let waits = request
            .get("request")
            .and_then(|inner| inner.get("subtype"))
            .and_then(Value::as_str)
            .is_some_and(|subtype| subtype == "can_use_tool" || subtype == "request_user_dialog");
        if waits {
            let key = request_key(request.get("request_id"));
            let prompt = Value::Object(request.clone());
            let mut core = self.shared.core();
            match core.pending_prompts.iter_mut().find(|(pending, _)| *pending == key) {
                Some(entry) => entry.1 = prompt,
                None => core.pending_prompts.push((key, prompt)),
            }
        }
        let frame = self.with_session_id(request);
        self.queue_frame(frame);
    }

    /// `sendControlResponse`: the prompt it answers is no longer pending, closed session or not.
    pub fn send_control_response(&mut self, response: Map<String, Value>) {
        let key = request_key(response.get("response").and_then(|inner| inner.get("request_id")));
        self.shared
            .core()
            .pending_prompts
            .retain(|(pending, _)| *pending != key);
        if self.is_closed() {
            return;
        }
        let frame = self.with_session_id(response);
        self.queue_frame(frame);
    }

    /// `sendControlCancelRequest`.
    pub fn send_control_cancel_request(&mut self, request_id: &str) {
        let key = request_key(Some(&Value::String(request_id.to_owned())));
        self.shared
            .core()
            .pending_prompts
            .retain(|(pending, _)| *pending != key);
        if self.is_closed() {
            return;
        }
        self.queue_frame(object([
            ("type", text("control_cancel_request")),
            ("request_id", text(request_id)),
            ("session_id", Value::String(self.session_id.clone())),
        ]));
    }

    /// `reportState`.
    pub fn report_state(&mut self, state: SessionState) {
        if !self.is_closed() {
            self.worker.submit_state(state, None);
            self.uploads.wake();
        }
    }

    /// `reportMetadata`.
    pub fn report_metadata(&mut self, metadata: &Map<String, Value>) {
        if !self.is_closed() {
            self.worker.submit_metadata(metadata);
            self.uploads.wake();
        }
    }

    /// `reportDelivery`: how far the host got with an inbound event.
    pub async fn report_delivery(&mut self, event_id: &str, status: DeliveryStatus) {
        if self.is_closed() {
            return;
        }
        let reported = self.worker.report_delivery(event_id, status).await;
        self.settle(reported);
        self.uploads.wake();
    }

    /// `flush`: sends the events the session holds. The reports and acknowledgements are not part of it.
    pub async fn flush(&mut self) {
        self.uploads.flush_window();
        // The post the uploader had started and the rows queued behind it are separate posts, as upstream.
        loop {
            let events = self.uploads.take_next();
            if events.is_empty() {
                break;
            }
            let submitted = self.worker.submit_events(events).await;
            self.settle(submitted);
            let flushed = self.worker.flush_events().await;
            self.settle(flushed);
        }
        let flushed = self.worker.flush_events().await;
        self.settle(flushed);
    }
}

impl<H, P> BridgeSession<H, P>
where
    H: HttpExecute + Clone + Send + Sync + 'static,
    P: WorkerPacing + Clone + Send + Sync + 'static,
{
    /// `reconnectTransport`: the old worker is left without a goodbye and a new one takes over from the sequence
    /// number the old read stream reached. What the old one had not delivered moves to the new one, and the writes
    /// the host made are remembered so the service does not hand them back.
    ///
    /// # Errors
    ///
    /// [`ReconnectError::InvalidUrl`] when the session URL cannot be parsed: what the old worker had not delivered
    /// is dropped and the session is closed, without a close code for the host.
    pub async fn reconnect_transport(&mut self, options: ReconnectOptions) -> Result<(), ReconnectError> {
        self.rebuild(options, false).await
    }

    /// `reconnectTransport` with `close` called while the rebuild is awaited: the transport is built, found closed
    /// and closed again, without registering and without carrying anything over.
    ///
    /// # Errors
    ///
    /// As [`BridgeSession::reconnect_transport`].
    pub async fn reconnect_transport_closed_meanwhile(
        &mut self,
        options: ReconnectOptions,
    ) -> Result<(), ReconnectError> {
        self.rebuild(options, true).await
    }

    async fn rebuild(&mut self, options: ReconnectOptions, closed_meanwhile: bool) -> Result<(), ReconnectError> {
        if self.is_closed() {
            return Ok(());
        }
        self.drain_outbox().await;
        let mut waiting = self.uploads.close_window();
        let held = self.worker.submit_events(waiting.clone()).await;
        self.settle(held);
        if !waiting.is_empty() && !matches!(self.worker.pump_events(Some(1)).await, Ok(true)) {
            waiting.clear();
        }
        self.token = options.ingress_token.clone();
        let sequence = self.shared.stream().last_sequence();
        if let Some(task) = self.stream_task.take() {
            task.abort();
        }
        self.shared.set_heartbeat(None);
        let epoch = options.epoch.unwrap_or_else(|| self.worker.epoch());
        self.worker.close_queue();
        self.worker.close_events(true);
        self.worker.close_deliveries();
        let url = session_url(&options.api_base_url, &self.session_id)?;
        if url::Url::parse(&url).is_err() {
            let _dropped = self.worker.take_undelivered_events();
            self.lifecycle.mark_closed();
            return Err(ReconnectError::InvalidUrl);
        }
        self.api_base_url = options.api_base_url;
        let rebuilt = WorkerClient::new(
            self.http.clone(),
            self.pacing.clone(),
            WorkerConfig {
                api_base_url: self.api_base_url.clone(),
                session_id: self.session_id.clone(),
                token: self.token.clone(),
                entrypoint: self.entrypoint.clone(),
                heartbeat_interval_ms: self.heartbeat_interval_ms,
                session_access_token: self.session_access_token.clone(),
                non_origin_403: NonOrigin403::Counted,
            },
        )?;
        let previous = std::mem::replace(&mut self.worker, Arc::new(rebuilt));
        self.worker_tx.send_replace(Arc::clone(&self.worker));
        if closed_meanwhile {
            self.mark_closed();
            wind_down(self.worker.as_ref());
            self.lifecycle.set_connected(false);
            self.uploads.wake();
            return Ok(());
        }
        *self.shared.stream() = StreamSession::seeded(Some(sequence));
        self.lifecycle.revive();
        self.connect(epoch).await;
        self.start_runtime();
        let mut carried: Vec<ClientEvent> = previous
            .take_undelivered_events()
            .into_iter()
            .filter(|event| !event.ephemeral)
            .collect();
        carried.extend(waiting);
        for event in &carried {
            let kind = event.payload.get("type").and_then(Value::as_str).unwrap_or_default();
            if NOT_CARRIED_AS_ECHO.contains(&kind) {
                continue;
            }
            if let Some(uuid) = event.payload.get("uuid").and_then(Value::as_str) {
                self.shared.core().router.remember_echo(uuid);
            }
        }
        let adopted = self.worker.submit_events(carried).await;
        self.settle(adopted);
        self.uploads.wake();
        Ok(())
    }
}
