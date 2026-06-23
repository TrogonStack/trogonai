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

use crate::agent::Bridge;
use crate::error::AGENT_UNAVAILABLE;
use crate::in_flight_slot_guard::InFlightSlotGuard;
use crate::jsonrpc::extract_request_id;
use crate::nats::{ClientMethod, FlushClient, PublishClient, RequestClient, SubscribeClient, parse_client_subject};
use agent_client_protocol::{Client, ErrorCode};
use async_nats::Message;
use bytes::Bytes;
use futures::StreamExt;
use std::cell::Cell;
use std::rc::Rc;
use tracing::{Span, error, info, instrument, warn};
use trogon_std::JsonSerialize;
use trogon_std::time::GetElapsed;

async fn publish_backpressure_error_reply<N: PublishClient + FlushClient, S: JsonSerialize>(
    nats: &N,
    payload: &[u8],
    reply_to: &str,
    serializer: &S,
) {
    let request_id = extract_request_id(payload);
    let (bytes, content_type) = rpc_reply::error_response_bytes(
        serializer,
        request_id,
        ErrorCode::Other(AGENT_UNAVAILABLE),
        "Client proxy overloaded; retry with backoff",
    );
    rpc_reply::publish_reply(nats, reply_to, bytes, content_type, "backpressure error reply").await;
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
    C: GetElapsed + 'static,
    J: 'static,
    S: Clone + JsonSerialize + 'static,
>(
    nats: N,
    client: Rc<Cl>,
    bridge: Rc<Bridge<N, C, J>>,
    serializer: S,
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
        process_message(
            msg,
            &nats,
            client.clone(),
            bridge.clone(),
            &in_flight,
            max_concurrent,
            &serializer,
        )
        .await;
    }

    info!("Client proxy subscriber ended");
}

async fn process_message<
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    Cl: Client + 'static,
    C: GetElapsed + 'static,
    J: 'static,
    S: Clone + JsonSerialize + 'static,
>(
    msg: Message,
    nats: &N,
    client: Rc<Cl>,
    bridge: Rc<Bridge<N, C, J>>,
    in_flight: &Rc<Cell<usize>>,
    max_concurrent: usize,
    serializer: &S,
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
            publish_backpressure_error_reply(nats, &payload, reply_to, serializer).await;
        }
        return;
    }
    let nats = nats.clone();
    let serializer = serializer.clone();
    let in_flight_guard = InFlightSlotGuard::new(in_flight.clone());
    tokio::task::spawn_local(async move {
        let _in_flight_guard = in_flight_guard;
        let ctx = DispatchContext {
            nats: &nats,
            client: client.as_ref(),
            bridge: bridge.as_ref(),
            serializer: &serializer,
        };
        dispatch_client_method(&subject, parsed, payload, reply, &ctx).await;
    });
}

struct DispatchContext<'a, N, Cl, C, J, S>
where
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    C: GetElapsed + 'static,
{
    nats: &'a N,
    client: &'a Cl,
    bridge: &'a Bridge<N, C, J>,
    serializer: &'a S,
}

#[instrument(skip(payload, ctx), fields(subject = %subject, session_id = tracing::field::Empty))]
async fn dispatch_client_method<
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    Cl: Client,
    C: GetElapsed + 'static,
    J: 'static,
    S: JsonSerialize,
>(
    subject: &str,
    parsed: crate::nats::ParsedClientSubject,
    payload: Bytes,
    reply: Option<String>,
    ctx: &DispatchContext<'_, N, Cl, C, J, S>,
) {
    Span::current().record("session_id", parsed.session_id.as_str());

    match parsed.method {
        ClientMethod::FsReadTextFile => {
            fs_read_text_file::handle(
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
                ctx.serializer,
            )
            .await;
        }
        ClientMethod::FsWriteTextFile => {
            fs_write_text_file::handle(
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
                ctx.serializer,
            )
            .await;
        }
        ClientMethod::SessionRequestPermission => {
            request_permission::handle(
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
                ctx.serializer,
            )
            .await;
        }
        ClientMethod::SessionUpdate => {
            session_update::handle(&payload, ctx.client, reply.is_some()).await;
        }
        ClientMethod::ExtSessionPromptResponse => {
            ext_session_prompt_response::handle(parsed.session_id.as_str(), &payload, reply.as_deref(), ctx.bridge)
                .await;
        }
        ClientMethod::TerminalCreate => {
            terminal_create::handle(
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
                ctx.serializer,
            )
            .await;
        }
        ClientMethod::TerminalKill => {
            terminal_kill::handle(
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
                ctx.serializer,
            )
            .await;
        }
        ClientMethod::TerminalOutput => {
            terminal_output::handle(
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
                ctx.serializer,
            )
            .await;
        }
        ClientMethod::TerminalRelease => {
            terminal_release::handle(
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
                ctx.serializer,
            )
            .await;
        }
        ClientMethod::TerminalWaitForExit => {
            terminal_wait_for_exit::handle(
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
                ctx.bridge.config.operation_timeout(),
                ctx.serializer,
            )
            .await;
        }
        ClientMethod::Ext(ref method_name) => {
            ext::handle(
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                method_name,
                ctx.serializer,
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests;
