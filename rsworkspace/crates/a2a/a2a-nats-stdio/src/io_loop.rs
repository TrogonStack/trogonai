use std::sync::Arc;

use a2a_nats::client::A2aClient;
use jsonrpc_nats::{CodecError, Message, RequestId, ResponseId, from_json_value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use tracing::{debug, error, warn};
use trogon_nats::RequestClient;
use trogon_nats::jetstream::{JetStreamCreateConsumer, JetStreamGetStream, JsAck, JsMessageOf, JsMessageRef};

use crate::constants::{CHANNEL_CAP, MAX_INFLIGHT_DISPATCH};
use crate::dispatch::dispatch_request;
use crate::wire::OutboundFrame;

/// Returns `Err` when the stdout writer task failed (broken pipe, write/flush
/// error). Callers should propagate so the process exits non-zero — a stdio
/// bridge whose downstream parent stopped reading should not pretend success.
pub async fn run_io_loop<N, J, R, W>(
    client: A2aClient<N, J>,
    stdin: R,
    mut stdout: W,
    shutdown: impl std::future::Future<Output = ()>,
) -> std::io::Result<()>
where
    N: RequestClient + Clone + Send + Sync + 'static,
    J: JetStreamGetStream + Clone + Send + Sync + 'static,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (frame_tx, mut frame_rx) = mpsc::channel::<OutboundFrame>(CHANNEL_CAP);

    let mut writer_task = tokio::spawn(async move {
        while let Some(frame) = frame_rx.recv().await {
            // serde_json::to_string failing means the dispatch task already
            // enqueued a frame the caller will never see. Surface it as an
            // io::Error so the main loop tears down instead of letting the
            // stdin caller hang waiting for a reply that was silently dropped.
            let json = serde_json::to_string(&frame).map_err(|e| {
                error!(error = %e, "frame serialization failed");
                std::io::Error::other(e)
            })?;
            if let Err(e) = stdout.write_all(json.as_bytes()).await {
                error!(error = %e, "stdout write failed");
                return Err(e);
            }
            if let Err(e) = stdout.write_all(b"\n").await {
                error!(error = %e, "stdout write failed");
                return Err(e);
            }
            // Flush per frame so a piped parent doesn't deadlock waiting on a
            // libc full-buffer that never drains until the next line on stdin
            // closes the loop.
            if let Err(e) = stdout.flush().await {
                error!(error = %e, "stdout flush failed");
                return Err(e);
            }
        }
        Ok::<(), std::io::Error>(())
    });

    let client = Arc::new(client);
    let mut lines = BufReader::new(stdin).lines();
    let semaphore = Arc::new(Semaphore::new(MAX_INFLIGHT_DISPATCH));
    let mut dispatch_tasks: JoinSet<()> = JoinSet::new();

    tokio::pin!(shutdown);

    // True iff we exited the read loop via the shutdown signal (signal-driven
    // teardown — abort in-flight stream dispatchers). False iff stdin closed
    // cleanly (drain in-flight RPCs).
    let mut shutdown_requested = false;

    // Set when the writer task exits — the loop must stop spawning dispatches
    // whose responses would land in a closed channel. None until detected.
    let mut writer_err: Option<std::io::Error> = None;
    // Surface stdin read failures / JoinSet failures alongside the writer
    // result so callers see the first real error rather than a silent Ok.
    let mut loop_err: Option<std::io::Error> = None;

    'outer: loop {
        tokio::select! {
            // `biased;` keeps shutdown priority once the signal fires — without
            // it, tokio's random branch selection can keep picking buffered
            // stdin lines (or EOF) and `shutdown_requested` would never flip,
            // so signal teardown would silently drain streaming tasks instead
            // of aborting them.
            biased;
            _ = &mut shutdown => {
                debug!("io_loop received shutdown signal");
                shutdown_requested = true;
                break 'outer;
            }
            res = &mut writer_task => {
                // Writer died — no point reading more stdin or dispatching;
                // responses would be silently dropped. Record the error and
                // tear down with abort semantics.
                writer_err = Some(writer_task_err(res));
                shutdown_requested = true;
                break 'outer;
            }
            line = lines.next_line() => {
                match line {
                    Err(e) => {
                        error!(error = %e, "stdin read error");
                        loop_err = Some(e);
                        shutdown_requested = true;
                        break 'outer;
                    }
                    Ok(None) => {
                        debug!("stdin closed");
                        break 'outer;
                    }
                    Ok(Some(raw)) => {
                        let raw = raw.trim().to_owned();
                        if raw.is_empty() {
                            continue;
                        }
                        debug!(raw = %raw, "received line");

                        let (id, method, params) = match parse_inbound(&raw) {
                            Ok(t) => t,
                            Err(err) => {
                                // Outbound channel may be full while writer is
                                // back-pressured. Poll the writer handle so
                                // its death also unsticks us, and stay
                                // responsive to shutdown.
                                tokio::select! {
                                    biased;
                                    _ = &mut shutdown => {
                                        shutdown_requested = true;
                                        break 'outer;
                                    }
                                    res = &mut writer_task => {
                                        writer_err = Some(writer_task_err(res));
                                        shutdown_requested = true;
                                        break 'outer;
                                    }
                                    _ = frame_tx.send(*err) => {}
                                }
                                continue;
                            }
                        };

                        // Acquire a dispatch slot before spawning so a fast
                        // producer can't create unbounded in-flight RPC work.
                        // Poll shutdown and writer-death alongside acquire so
                        // a saturated semaphore (e.g. permits held by
                        // long-lived streams) doesn't strand either signal
                        // when stdout breaks or shutdown fires.
                        let permit = tokio::select! {
                            biased;
                            _ = &mut shutdown => {
                                shutdown_requested = true;
                                break 'outer;
                            }
                            res = &mut writer_task => {
                                writer_err = Some(writer_task_err(res));
                                shutdown_requested = true;
                                break 'outer;
                            }
                            p = semaphore.clone().acquire_owned() => match p {
                                Ok(p) => p,
                                Err(e) => {
                                    error!(error = %e, "dispatch semaphore closed");
                                    break 'outer;
                                }
                            }
                        };
                        let client = client.clone();
                        let tx = frame_tx.clone();
                        dispatch_tasks.spawn(async move {
                            dispatch_request(&client, id, &method, params, &tx).await;
                            drop(permit);
                        });
                    }
                }
            }
        }
    }

    // On signal-driven shutdown, abort in-flight dispatchers — long-lived
    // `message/stream` / `tasks/resubscribe` tasks otherwise keep a `tx`
    // clone alive and the writer never sees frame_rx close. On clean EOF
    // we let in-flight RPCs drain so their responses reach stdout.
    if shutdown_requested {
        dispatch_tasks.abort_all();
    }
    while let Some(res) = dispatch_tasks.join_next().await {
        // Aborted joins surface as JoinError on signal-driven teardown; only
        // record join errors when shutdown wasn't requested so we don't
        // mistake our own abort for a real failure.
        if !shutdown_requested && let Err(join_err) = res {
            loop_err.get_or_insert_with(|| std::io::Error::other(join_err));
        }
    }
    drop(frame_tx);

    // If the writer-died branch fired, writer_err already holds its exit
    // status — the writer_task handle is consumed. Otherwise await it so any
    // frames still queued (e.g. dispatch responses produced just before a
    // stdin read error) finish flushing before we return.
    let writer_result = if writer_err.is_some() {
        writer_err.map_or(Ok(()), Err)
    } else {
        match writer_task.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(join_err) => Err(std::io::Error::other(join_err)),
        }
    };

    // Surface the first real failure: writer error wins (it means stdout was
    // broken or queued frames were lost), then loop errors (stdin read /
    // dispatch join), then success.
    match (writer_result, loop_err) {
        (Err(e), _) => Err(e),
        (Ok(()), Some(e)) => Err(e),
        (Ok(()), None) => Ok(()),
    }
}

/// Collapse the writer task's `Result<Result<(), io::Error>, JoinError>` into a
/// single `io::Error`. Used by every select arm that polls `&mut writer_task`.
fn writer_task_err(res: Result<std::io::Result<()>, tokio::task::JoinError>) -> std::io::Error {
    match res {
        Ok(Ok(())) => std::io::Error::other("writer task exited unexpectedly"),
        Ok(Err(e)) => e,
        Err(join_err) => std::io::Error::other(join_err),
    }
}

/// Split JSON-syntax failures (`-32700` Parse error) from envelope-shape
/// failures (`-32600` Invalid Request). JSON-RPC reserves `-32700` for actual
/// invalid JSON; structurally invalid requests are a different class.
fn parse_inbound(raw: &str) -> Result<(RequestId, String, serde_json::Value), Box<OutboundFrame>> {
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|e| {
        warn!(error = %e, "stdin line is not valid JSON");
        Box::new(OutboundFrame::error(
            ResponseId::Null,
            -32700,
            format!("parse error: {e}"),
        ))
    })?;
    // Salvage the request id from the raw JSON before the envelope check so a
    // malformed-shape `-32600` reply still correlates with the originating
    // call. JSON-RPC requires echoing the id when it can be determined.
    let salvaged_id = value
        .get("id")
        .and_then(|v| serde_json::from_value::<ResponseId>(v.clone()).ok())
        .unwrap_or(ResponseId::Null);
    let invalid_request = |message: String| {
        warn!(message, "JSON-RPC envelope is invalid");
        Box::new(OutboundFrame::error(salvaged_id.clone(), -32600, message))
    };
    match from_json_value(&value) {
        Ok(Message::Request { id, method, params }) => Ok((id, method, params)),
        // A notification carries no id, so its reply could never be correlated;
        // the stdio bridge answers requests only.
        Ok(_) => Err(invalid_request(
            "invalid request: expected a JSON-RPC request".to_string(),
        )),
        Err(CodecError::UnsupportedVersion { .. }) => Err(invalid_request(
            "invalid request: missing or unsupported jsonrpc version".to_string(),
        )),
        Err(e) => Err(invalid_request(format!("invalid request: {e}"))),
    }
}

#[cfg(test)]
mod parse_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod writer_err_tests;
