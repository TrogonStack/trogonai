//! `BridgeSessionHandle` (offset 1198000): what the host calls on an attached session. The writes queue behind the
//! control replies the session itself produced and go out with [`BridgeSession::pump`]; the upstream calls return
//! at once and upload in the background, which is the same thing seen from the caller.
//!

use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::SensitiveString;
use crate::adapters::HttpExecute;
use crate::bridge_session::{BridgeSession, request_key};
use crate::bridge_session_runtime::wind_down;
use crate::bridge_types::SessionState;
use crate::bridge_worker::{InvalidSessionId, NonOrigin403, WorkerClient, WorkerConfig, WorkerPacing, session_url};
use crate::bridge_worker_delivery::DeliveryStatus;
use crate::bridge_worker_events::ClientEvent;
use crate::bridge_worker_stream::StreamSession;

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
    InvalidSessionId(#[from] InvalidSessionId),
    /// `new URL` threw on the session URL built from the given base. The old worker's undelivered events are
    /// dropped and the session is closed without telling the host.
    #[error("Invalid URL")]
    InvalidUrl,
}

/// The frame types a rebuilt transport does not take back as its own writes.
const NOT_CARRIED_AS_ECHO: [&str; 4] = [
    "control_request",
    "control_response",
    "control_cancel_request",
    "result",
];

/// `Jp`: the usage block of a result nobody ran a turn for.
fn zero_usage() -> Value {
    json!({
        "output_tokens_details": {"thinking_tokens": 0},
        "input_tokens": 0,
        "cache_creation_input_tokens": 0,
        "cache_read_input_tokens": 0,
        "output_tokens": 0,
        "server_tool_use": {"web_search_requests": 0, "web_fetch_requests": 0},
        "service_tier": "standard",
        "cache_creation": {"ephemeral_1h_input_tokens": 0, "ephemeral_5m_input_tokens": 0},
        "inference_geo": "",
        "iterations": [],
        "speed": "standard",
        "fallback_credit": null,
    })
}

impl<H: HttpExecute, P: WorkerPacing> BridgeSession<H, P> {
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
            && !self.router.remember_echo(uuid)
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
        let frame = json!({
            "type": "result",
            "duration_ms": 0,
            "duration_api_ms": 0,
            "total_cost_usd": 0,
            "usage": zero_usage(),
            "modelUsage": {},
            "permission_denials": [],
            "session_id": self.session_id,
            "uuid": uuid::Uuid::new_v4().to_string(),
            "subtype": "success",
            "is_error": false,
            "num_turns": 0,
            "result": "",
            "stop_reason": null,
        });
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
            match self.pending_prompts.iter_mut().find(|(pending, _)| *pending == key) {
                Some(entry) => entry.1 = prompt,
                None => self.pending_prompts.push((key, prompt)),
            }
        }
        let frame = self.with_session_id(request);
        self.queue_frame(frame);
    }

    /// `sendControlResponse`: the prompt it answers is no longer pending, closed session or not.
    pub fn send_control_response(&mut self, response: Map<String, Value>) {
        let key = request_key(response.get("response").and_then(|inner| inner.get("request_id")));
        self.pending_prompts.retain(|(pending, _)| *pending != key);
        if self.is_closed() {
            return;
        }
        let frame = self.with_session_id(response);
        self.queue_frame(frame);
    }

    /// `sendControlCancelRequest`.
    pub fn send_control_cancel_request(&mut self, request_id: &str) {
        let key = request_key(Some(&Value::String(request_id.to_owned())));
        self.pending_prompts.retain(|(pending, _)| *pending != key);
        if self.is_closed() {
            return;
        }
        self.queue_frame(json!({
            "type": "control_cancel_request",
            "request_id": request_id,
            "session_id": self.session_id,
        }));
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
        self.drain_outbox().await;
        let flushed = self.worker.flush_events().await;
        self.settle(flushed);
    }
}

impl<H, P> BridgeSession<H, P>
where
    H: HttpExecute + Clone,
    P: WorkerPacing + Clone,
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
        let sequence = self.stream.last_sequence();
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
        self.stream = StreamSession::seeded(Some(sequence));
        self.lifecycle.revive();
        self.connect(epoch).await;
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
                self.router.remember_echo(uuid);
            }
        }
        let adopted = self.worker.submit_events(carried).await;
        self.settle(adopted);
        self.uploads.wake();
        Ok(())
    }
}
