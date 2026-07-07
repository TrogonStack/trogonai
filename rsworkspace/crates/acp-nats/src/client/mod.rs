pub(crate) mod ext;
pub(crate) mod ext_session_prompt_response;
pub(crate) mod fs_read_text_file;
pub(crate) mod fs_write_text_file;
pub(crate) mod request_permission;
pub(crate) mod rpc_reply;
pub(crate) mod session_update;
pub(crate) mod terminal_create;
pub(crate) mod terminal_kill;
pub(crate) mod terminal_output;
pub(crate) mod terminal_release;
pub(crate) mod terminal_wait_for_exit;

#[cfg(test)]
pub(crate) mod test_support;

use crate::agent::Bridge;
use crate::error::AGENT_UNAVAILABLE;
use crate::in_flight_slot_guard::InFlightSlotGuard;
use crate::nats::{ClientMethod, FlushClient, PublishClient, RequestClient, SubscribeClient, parse_client_subject};
use crate::wire::{encode_agent_error, merge_jsonrpc_headers, response_id_from_request_headers};
use agent_client_protocol::{Client, ErrorCode};
use async_nats::Message;
use async_nats::header::HeaderMap;
use bytes::Bytes;
use futures::StreamExt;
use std::cell::Cell;
use std::rc::Rc;
use tracing::{Span, error, info, instrument, warn};
use trogon_semconv::attribute::SESSION_ID;
use trogon_semconv::span::DISPATCH_CLIENT_METHOD;

async fn publish_backpressure_error_reply<N: PublishClient + FlushClient>(
    nats: &N,
    headers: &HeaderMap,
    reply_to: &str,
) {
    let response_id = response_id_from_request_headers(headers);
    let error = agent_client_protocol::Error::new(
        ErrorCode::Other(AGENT_UNAVAILABLE).into(),
        "Client proxy overloaded; retry with backoff",
    );
    match encode_agent_error(response_id, &error) {
        Ok(encoded) => {
            let reply_headers = merge_jsonrpc_headers(crate::nats::headers_with_trace_context(), encoded.headers);
            if let Err(e) = nats
                .publish_with_headers(reply_to.to_string(), reply_headers, encoded.body)
                .await
            {
                warn!(error = %e, "Failed to publish backpressure error reply");
            }
            if let Err(e) = nats.flush().await {
                warn!(error = %e, "Failed to flush backpressure error reply");
            }
        }
        Err(e) => warn!(error = %e, "Failed to encode backpressure error reply"),
    }
}

/// Runs the client proxy, subscribing to client subjects and dispatching to handlers.
///
/// # Panics / Runtime requirement
/// This function uses [`tokio::task::spawn_local`] internally and **must** be called from within
/// a [`tokio::task::LocalSet`] (or any executor that supports `!Send` tasks). Calling it outside
/// a `LocalSet` will panic at runtime when the first message is dispatched.
pub async fn run<
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    Cl: Client + 'static,
    C: trogon_std::time::GetElapsed + 'static,
    J: 'static,
>(
    nats: N,
    client: Rc<Cl>,
    bridge: Rc<Bridge<N, C, J>>,
) {
    let wildcard = crate::nats::subscriptions::AllClientSubject::new(bridge.config.acp_prefix_ref());
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
        process_message(msg, &nats, client.clone(), bridge.clone(), &in_flight, max_concurrent).await;
    }

    info!("Client proxy subscriber ended");
}

async fn process_message<
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    Cl: Client + 'static,
    C: trogon_std::time::GetElapsed + 'static,
    J: 'static,
>(
    msg: Message,
    nats: &N,
    client: Rc<Cl>,
    bridge: Rc<Bridge<N, C, J>>,
    in_flight: &Rc<Cell<usize>>,
    max_concurrent: usize,
) {
    let subject = msg.subject.to_string();

    // Validate subject before backpressure so unrecognised methods always
    // get InvalidParams, not a misleading "Bridge overloaded" error.
    let parsed = match parse_client_subject(&subject) {
        Some(parsed) => parsed,
        None => {
            warn!(subject = %subject, "Failed to parse client subject");
            return;
        }
    };

    let payload = msg.payload.clone();
    let reply = msg.reply.as_ref().map(|r| r.to_string());
    let headers = msg.headers.clone().unwrap_or_default();

    let current_in_flight = in_flight.get();
    if current_in_flight >= max_concurrent {
        warn!(
            in_flight = current_in_flight,
            method = ?parsed.method,
            subject = %subject,
            "Client task backpressure — rejecting message"
        );
        bridge.metrics.record_error("client", "client_backpressure_rejected");

        if let Some(reply_to) = &reply {
            publish_backpressure_error_reply(nats, &headers, reply_to).await;
        }
        return;
    }
    let nats = nats.clone();
    let in_flight_guard = InFlightSlotGuard::new(in_flight.clone());
    tokio::task::spawn_local(async move {
        let _in_flight_guard = in_flight_guard;
        let ctx = DispatchContext {
            nats: &nats,
            client: client.as_ref(),
            bridge: bridge.as_ref(),
        };
        dispatch_client_method(&subject, parsed, &headers, payload, reply, &ctx).await;
    });
}

struct DispatchContext<'a, N, Cl, C, J>
where
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    C: trogon_std::time::GetElapsed + 'static,
{
    nats: &'a N,
    client: &'a Cl,
    bridge: &'a Bridge<N, C, J>,
}

#[instrument(name = DISPATCH_CLIENT_METHOD, skip(headers, payload, ctx), fields(subject = %subject, session_id = tracing::field::Empty))]
async fn dispatch_client_method<
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    Cl: Client,
    C: trogon_std::time::GetElapsed + 'static,
    J: 'static,
>(
    subject: &str,
    parsed: crate::nats::ParsedClientSubject,
    headers: &HeaderMap,
    payload: Bytes,
    reply: Option<String>,
    ctx: &DispatchContext<'_, N, Cl, C, J>,
) {
    Span::current().record(SESSION_ID, parsed.session_id.as_str());

    match parsed.method {
        ClientMethod::FsReadTextFile => {
            fs_read_text_file::handle(
                headers,
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
            )
            .await;
        }
        ClientMethod::FsWriteTextFile => {
            fs_write_text_file::handle(
                headers,
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
            )
            .await;
        }
        ClientMethod::SessionRequestPermission => {
            request_permission::handle(
                headers,
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
            )
            .await;
        }
        ClientMethod::SessionUpdate => {
            session_update::handle(headers, &payload, ctx.client, reply.is_some(), &ctx.bridge.metrics).await;
        }
        ClientMethod::ExtSessionPromptResponse => {
            ext_session_prompt_response::handle(
                parsed.session_id.as_str(),
                headers,
                &payload,
                reply.as_deref(),
                ctx.bridge,
            )
            .await;
        }
        ClientMethod::TerminalCreate => {
            terminal_create::handle(
                headers,
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
            )
            .await;
        }
        ClientMethod::TerminalKill => {
            terminal_kill::handle(
                headers,
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
            )
            .await;
        }
        ClientMethod::TerminalOutput => {
            terminal_output::handle(
                headers,
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
            )
            .await;
        }
        ClientMethod::TerminalRelease => {
            terminal_release::handle(
                headers,
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
            )
            .await;
        }
        ClientMethod::TerminalWaitForExit => {
            terminal_wait_for_exit::handle(
                headers,
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
                ctx.bridge.config.operation_timeout(),
            )
            .await;
        }
        ClientMethod::Ext(ref method_name) => {
            ext::handle(headers, &payload, ctx.client, reply.as_deref(), ctx.nats, method_name).await;
        }
    }
}

#[cfg(test)]
mod tests;
