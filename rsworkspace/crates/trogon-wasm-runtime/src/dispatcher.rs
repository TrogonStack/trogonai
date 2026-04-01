use crate::runtime::WasmRuntime;
use acp_nats::nats::{ClientMethod, parse_client_subject};
use agent_client_protocol::{
    CreateTerminalRequest, KillTerminalRequest, ReadTextFileRequest, ReleaseTerminalRequest,
    RequestPermissionRequest, SessionNotification, TerminalOutputRequest,
    WaitForTerminalExitRequest, WriteTextFileRequest,
};
use async_nats::Client as NatsClient;
use bytes::Bytes;
use futures::StreamExt;
use std::rc::Rc;
use tracing::{debug, error, info, warn};

/// Subscribes to all ACP client subjects and dispatches to `WasmRuntime`.
///
/// Subject pattern: `{prefix}.session.*.client.>`
///
/// This loop is intentionally session-aware: each message carries the
/// `session_id` in its subject, which is forwarded to every runtime handler.
/// This is the key difference from `acp_nats::client::run()`, which uses
/// the `Client` trait and loses session context by the time methods are called.
pub async fn run(nats: NatsClient, prefix: String, runtime: Rc<WasmRuntime>) {
    let subject = format!("{prefix}.session.*.client.>");
    info!(%subject, "WASM runtime dispatcher subscribing");

    let mut sub = match nats.subscribe(subject.clone()).await {
        Ok(s) => s,
        Err(e) => {
            error!(error = %e, "Failed to subscribe to client subjects");
            return;
        }
    };

    while let Some(msg) = sub.next().await {
        let subject_str = msg.subject.to_string();
        let reply = msg.reply.clone();
        let payload = msg.payload.clone();

        let parsed = match parse_client_subject(&subject_str) {
            Some(p) => p,
            None => {
                warn!(%subject_str, "Could not parse client subject — ignoring");
                continue;
            }
        };

        let session_id = parsed.session_id.as_str().to_string();
        let method = parsed.method;
        let runtime = Rc::clone(&runtime);
        let nats = nats.clone();

        tokio::task::spawn_local(async move {
            dispatch(nats, session_id, method, payload, reply, runtime).await;
        });
    }

    info!("WASM runtime dispatcher exited");
}

async fn dispatch(
    nats: NatsClient,
    session_id: String,
    method: ClientMethod,
    payload: Bytes,
    reply: Option<async_nats::Subject>,
    runtime: Rc<WasmRuntime>,
) {
    match method {
        ClientMethod::TerminalCreate => {
            let req = match serde_json::from_slice::<CreateTerminalRequest>(&payload) {
                Ok(r) => r,
                Err(e) => {
                    reply_error(&nats, reply, -32600, &format!("Invalid request: {e}")).await;
                    return;
                }
            };
            let result = runtime.handle_create_terminal(&session_id, req).await;
            reply_result(&nats, reply, result).await;
        }

        ClientMethod::TerminalOutput => {
            let req = match serde_json::from_slice::<TerminalOutputRequest>(&payload) {
                Ok(r) => r,
                Err(e) => {
                    reply_error(&nats, reply, -32600, &format!("Invalid request: {e}")).await;
                    return;
                }
            };
            let result = runtime.handle_terminal_output(req).await;
            reply_result(&nats, reply, result).await;
        }

        ClientMethod::TerminalKill => {
            let req = match serde_json::from_slice::<KillTerminalRequest>(&payload) {
                Ok(r) => r,
                Err(e) => {
                    reply_error(&nats, reply, -32600, &format!("Invalid request: {e}")).await;
                    return;
                }
            };
            let result = runtime.handle_kill_terminal(req).await;
            reply_result(&nats, reply, result).await;
        }

        ClientMethod::TerminalRelease => {
            let req = match serde_json::from_slice::<ReleaseTerminalRequest>(&payload) {
                Ok(r) => r,
                Err(e) => {
                    reply_error(&nats, reply, -32600, &format!("Invalid request: {e}")).await;
                    return;
                }
            };
            let result = runtime.handle_release_terminal(req).await;
            reply_result(&nats, reply, result).await;
        }

        ClientMethod::TerminalWaitForExit => {
            let req = match serde_json::from_slice::<WaitForTerminalExitRequest>(&payload) {
                Ok(r) => r,
                Err(e) => {
                    reply_error(&nats, reply, -32600, &format!("Invalid request: {e}")).await;
                    return;
                }
            };
            let result = runtime.handle_wait_for_terminal_exit(req).await;
            reply_result(&nats, reply, result).await;
        }

        ClientMethod::FsWriteTextFile => {
            let req = match serde_json::from_slice::<WriteTextFileRequest>(&payload) {
                Ok(r) => r,
                Err(e) => {
                    reply_error(&nats, reply, -32600, &format!("Invalid request: {e}")).await;
                    return;
                }
            };
            let result = runtime.handle_write_text_file(&session_id, req).await;
            reply_result(&nats, reply, result).await;
        }

        ClientMethod::FsReadTextFile => {
            let req = match serde_json::from_slice::<ReadTextFileRequest>(&payload) {
                Ok(r) => r,
                Err(e) => {
                    reply_error(&nats, reply, -32600, &format!("Invalid request: {e}")).await;
                    return;
                }
            };
            let result = runtime.handle_read_text_file(&session_id, req).await;
            reply_result(&nats, reply, result).await;
        }

        ClientMethod::SessionRequestPermission => {
            let req = match serde_json::from_slice::<RequestPermissionRequest>(&payload) {
                Ok(r) => r,
                Err(e) => {
                    reply_error(&nats, reply, -32600, &format!("Invalid request: {e}")).await;
                    return;
                }
            };
            let result = runtime.handle_request_permission(req);
            reply_result(&nats, reply, result).await;
        }

        ClientMethod::SessionUpdate => {
            // Fire-and-forget notification — no reply expected.
            match serde_json::from_slice::<SessionNotification>(&payload) {
                Ok(notif) => runtime.handle_session_notification(notif),
                Err(e) => warn!(error = %e, "Failed to deserialize SessionNotification"),
            }
        }

        ClientMethod::ExtSessionPromptResponse | ClientMethod::Ext(_) => {
            // Not handled by this runtime — silently ignore.
            debug!(method = ?std::mem::discriminant(&method), "Unhandled client method");
        }
    }
}

async fn reply_result<T: serde::Serialize>(
    nats: &NatsClient,
    reply: Option<async_nats::Subject>,
    result: agent_client_protocol::Result<T>,
) {
    let Some(reply_to) = reply else { return };

    let body = match result {
        Ok(resp) => match serde_json::to_vec(&resp) {
            Ok(b) => b,
            Err(e) => {
                error!(error = %e, "Failed to serialize response");
                return;
            }
        },
        Err(err) => {
            let json = serde_json::json!({
                "jsonrpc": "2.0",
                "error": { "code": err.code, "message": err.message }
            });
            match serde_json::to_vec(&json) {
                Ok(b) => b,
                Err(e) => {
                    error!(error = %e, "Failed to serialize error response");
                    return;
                }
            }
        }
    };

    if let Err(e) = nats.publish(reply_to, body.into()).await {
        warn!(error = %e, "Failed to publish reply");
    }
}

async fn reply_error(
    nats: &NatsClient,
    reply: Option<async_nats::Subject>,
    code: i64,
    message: &str,
) {
    let Some(reply_to) = reply else { return };
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "error": { "code": code, "message": message }
    });
    if let Ok(b) = serde_json::to_vec(&body) {
        if let Err(e) = nats.publish(reply_to, b.into()).await {
            warn!(error = %e, "Failed to publish error reply");
        }
    }
}
