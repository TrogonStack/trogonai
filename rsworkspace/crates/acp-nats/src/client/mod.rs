pub(crate) mod ext_session_prompt_response;
pub(crate) mod fs_read_text_file;
pub(crate) mod fs_write_text_file;
pub(crate) mod request_permission;
pub(crate) mod session_update;
pub(crate) mod terminal_create;
pub(crate) mod terminal_kill;
pub(crate) mod terminal_output;
pub(crate) mod terminal_release;
pub(crate) mod terminal_wait_for_exit;

use crate::agent::Bridge;
use crate::nats::{
    ClientMethod, FlushClient, PublishClient, RequestClient, SubscribeClient, client,
    headers_with_trace_context, parse_client_subject,
};
use agent_client_protocol::Client;
use agent_client_protocol::ErrorCode;
use bytes::Bytes;
use futures::StreamExt;
use std::cell::Cell;
use std::rc::Rc;
use tracing::{Span, error, info, instrument, warn};
use trogon_std::time::GetElapsed;

struct InFlightSlotGuard(Rc<Cell<usize>>);

impl InFlightSlotGuard {
    fn new(counter: Rc<Cell<usize>>) -> Self {
        counter.set(counter.get().saturating_add(1));
        Self(counter)
    }
}

impl Drop for InFlightSlotGuard {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_sub(1));
    }
}

fn jsonrpc_error_response(request_id: serde_json::Value, code: ErrorCode, message: &str) -> Bytes {
    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "error": {
            "code": i32::from(code),
            "message": message
        }
    });
    serde_json::to_vec(&response).unwrap_or_else(|e| {
        format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{{\"code\":{},\"message\":\"failed to serialize error response: {}\"}}}}",
            i32::from(code),
            e
        )
        .into_bytes()
    })
    .into()
}

fn extract_request_id(payload: &[u8]) -> serde_json::Value {
    serde_json::from_slice::<serde_json::Value>(payload)
        .ok()
        .and_then(|v| v.get("id").cloned())
        .unwrap_or_else(|| {
            warn!(
                "Malformed or missing JSON-RPC request id in payload while handling client request"
            );
            serde_json::Value::Null
        })
}

pub async fn run<
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    Cl: Client + 'static,
    C: GetElapsed + 'static,
>(
    nats: N,
    client: Rc<Cl>,
    bridge: Rc<Bridge<N, C>>,
) {
    let wildcard = client::wildcards::all(&bridge.config.acp_prefix);
    info!("Starting client proxy - subscribing to {}", wildcard);

    let mut subscriber = match nats.subscribe(wildcard).await {
        Ok(sub) => sub,
        Err(e) => {
            error!(error = %e, "Failed to subscribe to client subjects");
            return;
        }
    };

    let in_flight = Rc::new(Cell::new(0usize));
    let max_concurrent = bridge.config.max_concurrent_client_tasks();

    while let Some(msg) = subscriber.next().await {
        let request_id = extract_request_id(&msg.payload);
        let subject = msg.subject.to_string();
        let parsed = parse_client_subject(&subject);
        let is_notification = parsed.as_ref().is_some_and(|p| {
            matches!(
                p.method,
                ClientMethod::SessionUpdate | ClientMethod::ExtSessionPromptResponse
            )
        });

        if !is_notification && in_flight.get() >= max_concurrent {
            warn!(
                in_flight = in_flight.get(),
                method = ?parsed.as_ref().map(|p| &p.method),
                subject = %subject,
                "Client task backpressure — rejecting message"
            );
            bridge.metrics.record_error("client", "client_backpressure_rejected");

            if let Some(reply_to) = msg.reply.as_ref().map(|reply| reply.to_string()) {
                let bytes = jsonrpc_error_response(
                    request_id,
                    ErrorCode::InternalError,
                    "Bridge overloaded - too many concurrent requests",
                );
                let headers = headers_with_trace_context();
                if let Err(e) = nats
                    .publish_with_headers(reply_to, headers, bytes)
                    .await
                {
                    error!(error = %e, "Failed to publish backpressure response");
                }
                if let Err(error) = nats.flush().await {
                    warn!(error = %error, "Failed to flush backpressure response");
                }
            } else {
                warn!(
                    subject = %subject,
                    in_flight = in_flight.get(),
                    "No reply_to on request; dropping due to backpressure"
                );
            }
            continue;
        }

        let parsed = match parsed {
            Some(parsed) => parsed,
            None => {
                warn!(subject = %subject, "Failed to parse client subject");
                if let Some(reply_to) = msg.reply.as_ref().map(|reply| reply.to_string()) {
                    let bytes = jsonrpc_error_response(
                        request_id,
                        ErrorCode::InvalidParams,
                        "Invalid client subject",
                    );
                    let headers = headers_with_trace_context();
                    let _ = nats
                        .publish_with_headers(reply_to, headers, bytes)
                        .await;
                }
                continue;
            }
        };

        let reply = msg.reply.as_ref().map(|reply| reply.to_string());
        let payload = msg.payload.clone();
        let nats = nats.clone();
        let client = client.clone();

        let bridge_clone = bridge.clone();
        let in_flight_guard = InFlightSlotGuard::new(in_flight.clone());
        tokio::task::spawn_local(async move {
            let _in_flight_guard = in_flight_guard;
            handle_client_request(
                &subject,
                parsed,
                payload,
                reply,
                &nats,
                client.as_ref(),
                bridge_clone.as_ref(),
            )
            .await;
        });
    }

    info!("Client proxy subscriber ended");
}

#[instrument(skip(payload, reply, nats, client, bridge), fields(subject = %subject, session_id = tracing::field::Empty))]
async fn handle_client_request<
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    Cl: Client,
    C: GetElapsed,
>(
    subject: &str,
    parsed: crate::nats::ParsedClientSubject,
    payload: Bytes,
    reply: Option<String>,
    nats: &N,
    client: &Cl,
    bridge: &Bridge<N, C>,
) {
    let request_id = extract_request_id(&payload);

    Span::current().record("session_id", parsed.session_id.as_str());

    info!(method = ?parsed.method, session_id = %parsed.session_id, "Handling client request");

    let response_bytes = match parsed.method {
        ClientMethod::FsReadTextFile => {
            fs_read_text_file::handle(&payload, client, bridge.config.max_nats_payload_bytes()).await
        }
        ClientMethod::FsWriteTextFile => {
            fs_write_text_file::handle(&payload, client, bridge.config.max_nats_payload_bytes())
                .await
        }
        ClientMethod::SessionRequestPermission => {
            request_permission::handle(&payload, client).await
        }
        ClientMethod::SessionUpdate => {
            info!(session_id = %parsed.session_id, "Forwarding session update to client");
            if reply.is_some() {
                warn!(
                    session_id = %parsed.session_id,
                    method = ?parsed.method,
                    "Unexpected reply subject on notification request"
                );
            }
            session_update::handle(&payload, client).await;
            return;
        }
        ClientMethod::ExtSessionPromptResponse => {
            if reply.is_some() {
                warn!(
                    session_id = %parsed.session_id,
                    method = ?parsed.method,
                    "Unexpected reply subject on prompt response notification"
                );
            }
            ext_session_prompt_response::handle(parsed.session_id.as_str(), &payload, bridge).await;
            return; // No reply needed for notifications
        }
        ClientMethod::TerminalCreate => terminal_create::handle(&payload, client).await,
        ClientMethod::TerminalKill => terminal_kill::handle(&payload, client).await,
        ClientMethod::TerminalOutput => terminal_output::handle(&payload, client).await,
        ClientMethod::TerminalRelease => terminal_release::handle(&payload, client).await,
        ClientMethod::TerminalWaitForExit => {
            terminal_wait_for_exit::handle(&payload, client, bridge.config.operation_timeout).await
        }
    };

    if let Some(reply_to) = reply {
        match response_bytes {
            Ok(bytes) => {
                let headers = headers_with_trace_context();
                if let Err(e) = nats
                    .publish_with_headers(reply_to, headers, bytes.into())
                    .await
                {
                    error!(error = %e, "Failed to publish reply");
                }
                if let Err(error) = nats.flush().await {
                    warn!(error = %error, "Failed to flush response");
                }
            }
            Err(e) => {
                error!(
                    error = %e,
                    method = ?parsed.method,
                    session_id = %parsed.session_id,
                    "Failed to handle client request"
                );
                let bytes = jsonrpc_error_response(
                    request_id,
                    ErrorCode::InternalError,
                    "Internal error while handling client request",
                );
                let headers = headers_with_trace_context();
                if let Err(e) = nats
                    .publish_with_headers(reply_to, headers, bytes)
                    .await
                {
                    error!(error = %e, "Failed to publish error reply");
                }
                if let Err(error) = nats.flush().await {
                    warn!(error = %error, "Failed to flush error reply");
                }
            }
        }
    }
}
