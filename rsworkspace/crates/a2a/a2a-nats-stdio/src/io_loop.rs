use std::sync::Arc;

use a2a_nats::client::A2aClient;
use jsonrpc_nats::{CodecError, Message, RequestId, ResponseId, from_json_value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
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
    stdout: W,
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
    let (frame_tx, frame_rx) = mpsc::channel::<OutboundFrame>(CHANNEL_CAP);
    let mut writer_task = tokio::spawn(write_frames(stdout, frame_rx));

    let client = Arc::new(client);
    let mut lines = BufReader::new(stdin).lines();
    let mut dispatch_tasks: JoinSet<()> = JoinSet::new();
    let mut dispatch_err = None;

    tokio::pin!(shutdown);

    let mut shutdown_requested = false;
    let mut completed_writer = None;
    let mut loop_err: Option<std::io::Error> = None;

    'outer: loop {
        tokio::select! {
            // Shutdown must win over buffered stdin and EOF so streams are aborted.
            biased;
            _ = &mut shutdown => {
                debug!("io_loop received shutdown signal");
                shutdown_requested = true;
                break 'outer;
            }
            res = &mut writer_task => {
                completed_writer = Some(writer_task_result(res));
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
                                tokio::select! {
                                    biased;
                                    _ = &mut shutdown => {
                                        shutdown_requested = true;
                                        break 'outer;
                                    }
                                    res = &mut writer_task => {
                                        completed_writer = Some(writer_task_result(res));
                                        shutdown_requested = true;
                                        break 'outer;
                                    }
                                    _ = frame_tx.send(*err) => {}
                                }
                                continue;
                            }
                        };

                        tokio::select! {
                            biased;
                            _ = &mut shutdown => {
                                shutdown_requested = true;
                                break 'outer;
                            }
                            res = &mut writer_task => {
                                completed_writer = Some(writer_task_result(res));
                                shutdown_requested = true;
                                break 'outer;
                            }
                            () = wait_for_dispatch_capacity(&mut dispatch_tasks, &mut dispatch_err) => {}
                        }
                        let client = client.clone();
                        let tx = frame_tx.clone();
                        dispatch_tasks.spawn(async move {
                            dispatch_request(&client, id, &method, params, &tx).await;
                        });
                    }
                }
            }
        }
    }

    // Signal teardown must release streaming senders; EOF must drain replies.
    if shutdown_requested {
        dispatch_tasks.abort_all();
    }
    while let Some(res) = dispatch_tasks.join_next().await {
        if let Err(error) = res {
            dispatch_err.get_or_insert(error);
        }
    }
    if !shutdown_requested && let Some(error) = dispatch_err {
        loop_err.get_or_insert_with(|| std::io::Error::other(error));
    }
    drop(frame_tx);

    let writer_result = match completed_writer {
        Some(result) => result,
        None => writer_task_result(writer_task.await),
    };

    // A failed writer means queued replies were lost, so it takes precedence.
    match (writer_result, loop_err) {
        (Err(e), _) => Err(e),
        (Ok(()), Some(e)) => Err(e),
        (Ok(()), None) => Ok(()),
    }
}

async fn wait_for_dispatch_capacity(tasks: &mut JoinSet<()>, dispatch_err: &mut Option<tokio::task::JoinError>) {
    // Reaping completed tasks bounds retained join records as well as active work.
    while tasks.len() >= MAX_INFLIGHT_DISPATCH {
        if let Some(Err(error)) = tasks.join_next().await {
            dispatch_err.get_or_insert(error);
        }
    }
}

async fn write_frames<W: tokio::io::AsyncWrite + Unpin>(
    mut stdout: W,
    mut frame_rx: mpsc::Receiver<OutboundFrame>,
) -> std::io::Result<()> {
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
    Ok(())
}

fn writer_task_result(res: Result<std::io::Result<()>, tokio::task::JoinError>) -> std::io::Result<()> {
    res.map_err(std::io::Error::other)?
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
