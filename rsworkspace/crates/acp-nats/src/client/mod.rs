mod ext_session_prompt_response;
mod fs_read_text_file;
mod fs_write_text_file;
mod request_permission;
mod session_update;
mod terminal_create;
mod terminal_kill;
mod terminal_output;
mod terminal_release;
mod terminal_wait_for_exit;

use crate::agent::Bridge;
use crate::nats::{
    ClientMethod, FlushClient, PublishClient, RequestClient, SubscribeClient, client,
    headers_with_trace_context, parse_client_subject,
};
use agent_client_protocol::Client;
use bytes::Bytes;
use futures::StreamExt;
use std::sync::Arc;
use tracing::{Span, error, info, instrument, warn};

pub async fn run<N: SubscribeClient + RequestClient + PublishClient + FlushClient, C: Client + Send + Sync + 'static>(
    nats: N,
    client: Arc<C>,
    bridge: Arc<Bridge<N>>,
) {
    let wildcard = client::wildcards::all(&bridge.acp_prefix);
    info!("Starting client proxy - subscribing to {}", wildcard);

    let subscriber = match nats.subscribe(wildcard).await {
        Ok(sub) => sub,
        Err(e) => {
            error!(error = %e, "Failed to subscribe to client subjects");
            return;
        }
    };

    let mut subscriber = subscriber;

    while let Some(msg) = subscriber.next().await {
        let subject = msg.subject.to_string();
        let reply = msg.reply.clone();
        let payload = msg.payload.clone();
        let nats = nats.clone();
        let client = client.clone();

        let bridge_clone = bridge.clone();
        tokio::task::spawn_local(async move {
            handle_client_request(
                &subject,
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
async fn handle_client_request<N: SubscribeClient + RequestClient + PublishClient + FlushClient, C: Client>(
    subject: &str,
    payload: Bytes,
    reply: Option<async_nats::Subject>,
    nats: &N,
    client: &C,
    bridge: &Bridge<N>,
) {
    let parsed = match parse_client_subject(subject) {
        Some(p) => p,
        None => {
            warn!(subject = %subject, "Failed to parse client subject");
            return;
        }
    };

    Span::current().record("session_id", &parsed.session_id);

    info!(method = ?parsed.method, session_id = %parsed.session_id, "Handling client request");

    let response_bytes = match parsed.method {
        ClientMethod::FsReadTextFile => fs_read_text_file::handle(&payload, client).await,
        ClientMethod::FsWriteTextFile => fs_write_text_file::handle(&payload, client).await,
        ClientMethod::SessionRequestPermission => {
            request_permission::handle(&payload, client).await
        }
        ClientMethod::SessionUpdate => {
            info!(session_id = %parsed.session_id, "Forwarding session update to client");
            session_update::handle(&payload, client).await;
            return;
        }
        ClientMethod::ExtSessionPromptResponse => {
            ext_session_prompt_response::handle(&parsed.session_id, &payload, bridge).await;
            return; // No reply needed for notifications
        }
        ClientMethod::TerminalCreate => terminal_create::handle(&payload, client).await,
        ClientMethod::TerminalKill => terminal_kill::handle(&payload, client).await,
        ClientMethod::TerminalOutput => terminal_output::handle(&payload, client).await,
        ClientMethod::TerminalRelease => terminal_release::handle(&payload, client).await,
        ClientMethod::TerminalWaitForExit => terminal_wait_for_exit::handle(&payload, client).await,
    };

    if let Some(reply_to) = reply {
        match response_bytes {
            Ok(bytes) => {
                let headers = headers_with_trace_context();
                if let Err(e) = nats
                    .publish_with_headers(reply_to.to_string(), headers, bytes.into())
                    .await
                {
                    error!(error = %e, "Failed to publish reply");
                }
            }
            Err(e) => {
                let error_response = serde_json::json!({
                    "error": {
                        "code": -32603,
                        "message": e.to_string()
                    }
                });
                let bytes = serde_json::to_vec(&error_response).unwrap_or_default();
                let headers = headers_with_trace_context();
                if let Err(e) = nats
                    .publish_with_headers(reply_to.to_string(), headers, bytes.into())
                    .await
                {
                    error!(error = %e, "Failed to publish error reply");
                }
            }
        }
    }
}
