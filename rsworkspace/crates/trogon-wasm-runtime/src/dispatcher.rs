use crate::WasmRuntime;
use acp_nats::nats::{parse_client_subject, ClientMethod};
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
///
/// `shutdown` is a watch channel receiver; when its value becomes `true` the
/// dispatcher drains in-flight tasks and exits cleanly.
///
/// # Trust model
///
/// Session isolation is enforced at the NATS subject layer, not inside the
/// runtime. A client can only publish to subjects under its own session
/// (`{prefix}.session.{its_session_id}.client.*`) if NATS per-client ACLs
/// are configured correctly. The runtime receives messages from all sessions
/// because it is the server side of the protocol — it does not perform
/// additional session-ownership checks. This is intentional: adding a
/// redundant check here would duplicate the NATS ACL boundary and add no
/// real security. Operators must configure NATS publish permissions to
/// restrict each client to its own session subject.
pub async fn run(
    nats: NatsClient,
    prefix: String,
    runtime: Rc<WasmRuntime>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let subject = format!("{prefix}.session.*.client.>");
    info!(%subject, "WASM runtime dispatcher subscribing");

    let mut sub = match nats.subscribe(subject.clone()).await {
        Ok(s) => s,
        Err(e) => {
            error!(error = %e, "Failed to subscribe to client subjects");
            return;
        }
    };

    // Spawn periodic idle-session cleanup task.
    let runtime_for_cleanup = Rc::clone(&runtime);
    let mut shutdown_for_cleanup = shutdown.clone();
    tokio::task::spawn_local(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    runtime_for_cleanup.cleanup_idle_sessions();
                }
                _ = shutdown_for_cleanup.changed() => {
                    if *shutdown_for_cleanup.borrow() {
                        break;
                    }
                }
            }
        }
    });

    // Spawn periodic metrics log task (every 5 minutes).
    let mut shutdown_for_metrics = shutdown.clone();
    tokio::task::spawn_local(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let snap = crate::metrics::METRICS.snapshot();
                    tracing::info!(
                        wasm_started = snap.wasm_tasks_started,
                        wasm_completed = snap.wasm_tasks_completed,
                        wasm_faulted = snap.wasm_tasks_faulted,
                        fuel_exhausted = snap.wasm_fuel_exhausted,
                        native_started = snap.native_tasks_started,
                        cache_hits = snap.cache_hits,
                        cache_misses = snap.cache_misses,
                        host_calls = snap.host_calls_total,
                        "runtime metrics"
                    );
                }
                _ = shutdown_for_metrics.changed() => {
                    if *shutdown_for_metrics.borrow() {
                        break;
                    }
                }
            }
        }
    });

    // Track in-flight dispatch tasks.
    let mut tasks: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

    loop {
        // Reap completed tasks to avoid unbounded growth.
        while tasks.try_join_next().is_some() {}

        tokio::select! {
            biased;

            // Check for shutdown signal first.
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    info!("Shutdown signal received in dispatcher — draining in-flight tasks");
                    break;
                }
            }

            msg = sub.next() => {
                let msg = match msg {
                    Some(m) => m,
                    None => {
                        info!("NATS subscription closed");
                        break;
                    }
                };

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

                tasks.spawn_local(async move {
                    dispatch(nats, session_id, method, payload, reply, runtime).await;
                });
            }
        }
    }

    // Drain all in-flight tasks before returning.
    info!(
        tasks = tasks.len(),
        "Waiting for in-flight tasks to complete"
    );
    tasks.join_all().await;

    // Clean up all sessions on graceful shutdown.
    runtime.cleanup_all_sessions().await;

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

        ClientMethod::ExtSessionPromptResponse => {
            debug!("ExtSessionPromptResponse — not handled");
            reply_error(&nats, reply, -32601, "Method not supported by this runtime").await;
        }

        ClientMethod::Ext(ref name) if name == "terminal.write_stdin" => {
            // Payload: { "terminal_id": "...", "data": [1, 2, 3, ...] }
            #[derive(serde::Deserialize)]
            struct WriteStdinRequest {
                terminal_id: String,
                /// Data as a JSON array of bytes, e.g. [104, 101, 108, 108, 111].
                data: Vec<u8>,
            }
            match serde_json::from_slice::<WriteStdinRequest>(&payload) {
                Ok(req) => {
                    let result = runtime
                        .handle_write_to_terminal(&req.terminal_id, &req.data)
                        .await;
                    reply_result(&nats, reply, result).await;
                }
                Err(e) => {
                    reply_error(&nats, reply, -32600, &format!("Invalid request: {e}")).await;
                }
            }
        }

        ClientMethod::Ext(ref name) if name == "terminal.close_stdin" => {
            // Payload: { "terminal_id": "..." }
            #[derive(serde::Deserialize)]
            struct CloseStdinRequest {
                terminal_id: String,
            }
            match serde_json::from_slice::<CloseStdinRequest>(&payload) {
                Ok(req) => {
                    let result = runtime.handle_close_terminal_stdin(&req.terminal_id);
                    reply_result(&nats, reply, result).await;
                }
                Err(e) => {
                    reply_error(&nats, reply, -32600, &format!("Invalid request: {e}")).await;
                }
            }
        }

        ClientMethod::Ext(ref name) if name == "runtime.list_sessions" => {
            let sessions = runtime.list_sessions();
            let body = serde_json::json!({ "sessions": sessions });
            if let Some(reply_to) = reply {
                if let Ok(b) = serde_json::to_vec(&body) {
                    if let Err(e) = nats.publish(reply_to, b.into()).await {
                        warn!(error = %e, "Failed to publish list_sessions reply");
                    }
                }
            }
        }

        ClientMethod::Ext(ref name) if name == "runtime.list_terminals" => {
            let terminals = runtime.list_terminals();
            let items: Vec<serde_json::Value> = terminals
                .into_iter()
                .map(|(tid, sid)| serde_json::json!({ "terminal_id": tid, "session_id": sid }))
                .collect();
            let body = serde_json::json!({ "terminals": items });
            if let Some(reply_to) = reply {
                if let Ok(b) = serde_json::to_vec(&body) {
                    if let Err(e) = nats.publish(reply_to, b.into()).await {
                        warn!(error = %e, "Failed to publish list_terminals reply");
                    }
                }
            }
        }

        ClientMethod::Ext(_) => {
            debug!("Unhandled Ext method");
            reply_error(&nats, reply, -32601, "Method not supported by this runtime").await;
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
