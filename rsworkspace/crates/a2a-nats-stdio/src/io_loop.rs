use std::sync::Arc;

use a2a_nats::client::A2aClient;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use tracing::{debug, error, warn};
use trogon_nats::RequestClient;
use trogon_nats::jetstream::{JetStreamCreateConsumer, JetStreamGetStream, JsAck, JsMessageOf, JsMessageRef};

use crate::dispatch::dispatch_request;
use crate::wire::{InboundRequest, OutboundError, OutboundFrame, RpcId};

const CHANNEL_CAP: usize = 128;
/// Cap concurrent in-flight dispatch tasks. A fast producer on stdin can
/// otherwise create unbounded RPC/network work and memory pressure.
const MAX_INFLIGHT_DISPATCH: usize = 64;

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
                                    _ = frame_tx.send(OutboundFrame::Error(err)) => {}
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
fn parse_inbound(raw: &str) -> Result<(RpcId, String, serde_json::Value), OutboundError> {
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|e| {
        warn!(error = %e, "stdin line is not valid JSON");
        OutboundError::new(RpcId::Null, -32700, format!("parse error: {e}"))
    })?;
    // Salvage the request id from the raw JSON before the envelope check so a
    // malformed-shape `-32600` reply still correlates with the originating
    // call. JSON-RPC requires echoing the id when it can be determined.
    let salvaged_id = value
        .get("id")
        .and_then(|v| serde_json::from_value::<RpcId>(v.clone()).ok())
        .unwrap_or(RpcId::Null);
    // InboundRequest deserializes only `id`/`method`/`params`, so a missing or
    // wrong `jsonrpc` would otherwise dispatch as a real call. JSON-RPC 2.0
    // requires the version field exactly equal to "2.0".
    if !matches!(value.get("jsonrpc"), Some(serde_json::Value::String(v)) if v == "2.0") {
        warn!("JSON-RPC envelope has missing or wrong version");
        return Err(OutboundError::new(
            salvaged_id,
            -32600,
            "invalid request: missing or unsupported jsonrpc version".to_string(),
        ));
    }
    let req: InboundRequest = serde_json::from_value(value).map_err(|e| {
        warn!(error = %e, "JSON-RPC envelope is invalid");
        OutboundError::new(salvaged_id, -32600, format!("invalid request: {e}"))
    })?;
    Ok((req.id, req.method, req.params))
}

#[cfg(test)]
mod tests {
    use super::*;
    use a2a_nats::client::A2aClient;
    use a2a_nats::{A2aAgentId, A2aPrefix};
    use bytes::Bytes;
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_nats::jetstream::mocks::MockJetStreamConsumerFactory;

    fn make_client(
        nats: AdvancedMockNatsClient,
        js: MockJetStreamConsumerFactory,
    ) -> A2aClient<AdvancedMockNatsClient, MockJetStreamConsumerFactory> {
        let prefix = A2aPrefix::new("a2a").unwrap();
        let agent_id = A2aAgentId::new("bot").unwrap();
        A2aClient::new(prefix, agent_id, nats, js)
    }

    fn task_response(task_id: &str) -> Bytes {
        let task = a2a::types::Task {
            id: task_id.to_string(),
            context_id: String::new(),
            status: a2a::types::TaskStatus {
                state: a2a::types::TaskState::Completed,
                message: None,
                timestamp: None,
            },
            artifacts: None,
            history: None,
            metadata: None,
        };
        serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0", "id": "x", "result": task
        }))
        .unwrap()
        .into()
    }

    #[tokio::test]
    async fn io_loop_exits_on_eof() {
        let nats = AdvancedMockNatsClient::new();
        let client = make_client(nats, MockJetStreamConsumerFactory::new());

        let (stdin_reader, _stdin_writer) = tokio::io::duplex(1024);
        let (_stdout_reader, stdout_writer) = tokio::io::duplex(1024);
        drop(_stdin_writer);

        let _ = run_io_loop(client, stdin_reader, stdout_writer, std::future::pending::<()>()).await;
    }

    #[tokio::test]
    async fn io_loop_exits_on_shutdown_signal() {
        let nats = AdvancedMockNatsClient::new();
        let client = make_client(nats, MockJetStreamConsumerFactory::new());

        let (stdin_reader, _stdin_writer) = tokio::io::duplex(1024);
        let (_stdout_reader, stdout_writer) = tokio::io::duplex(1024);

        let _ = run_io_loop(client, stdin_reader, stdout_writer, std::future::ready(())).await;
    }

    #[tokio::test]
    async fn io_loop_skips_blank_lines() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agents.bot.tasks.get", task_response("t-blank"));
        let client = make_client(nats, MockJetStreamConsumerFactory::new());

        let (stdin_reader, mut stdin_writer) = tokio::io::duplex(4096);
        let (mut stdout_reader, stdout_writer) = tokio::io::duplex(4096);

        // Two blank lines, then a real request, then EOF.
        stdin_writer.write_all(b"\n   \n").await.unwrap();
        stdin_writer
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tasks/get\",\"params\":{\"id\":\"t-blank\",\"tenant\":\"\"}}\n")
            .await
            .unwrap();
        drop(stdin_writer);

        let _ = run_io_loop(client, stdin_reader, stdout_writer, std::future::pending::<()>()).await;

        let mut buf = vec![0u8; 4096];
        let n = stdout_reader.read(&mut buf).await.unwrap();
        let output = String::from_utf8_lossy(&buf[..n]);
        // The blank lines are dropped silently and only the real request produces output.
        assert!(output.contains("t-blank"), "expected response, got: {output}");
        assert_eq!(output.matches('\n').count(), 1, "blank lines should not emit frames");
    }

    #[tokio::test]
    async fn io_loop_handles_valid_request() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agents.bot.tasks.get", task_response("t1"));
        let client = make_client(nats, MockJetStreamConsumerFactory::new());

        let (stdin_reader, mut stdin_writer) = tokio::io::duplex(4096);
        let (mut stdout_reader, stdout_writer) = tokio::io::duplex(4096);

        let request = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tasks/get\",\"params\":{\"id\":\"t1\",\"tenant\":\"\",\"historyLength\":null}}\n";
        stdin_writer.write_all(request).await.unwrap();
        drop(stdin_writer);

        let _ = run_io_loop(client, stdin_reader, stdout_writer, std::future::pending::<()>()).await;

        let mut buf = vec![0u8; 4096];
        let n = stdout_reader.read(&mut buf).await.unwrap();
        let output = String::from_utf8_lossy(&buf[..n]);
        assert!(output.contains("\"result\""), "expected result in: {output}");
        assert!(output.contains("t1"));
    }

    #[tokio::test]
    async fn io_loop_handles_parse_error() {
        let nats = AdvancedMockNatsClient::new();
        let client = make_client(nats, MockJetStreamConsumerFactory::new());

        let (stdin_reader, mut stdin_writer) = tokio::io::duplex(4096);
        let (mut stdout_reader, stdout_writer) = tokio::io::duplex(4096);

        stdin_writer.write_all(b"not valid json\n").await.unwrap();
        drop(stdin_writer);

        let _ = run_io_loop(client, stdin_reader, stdout_writer, std::future::pending::<()>()).await;

        let mut buf = vec![0u8; 4096];
        let n = stdout_reader.read(&mut buf).await.unwrap();
        let output = String::from_utf8_lossy(&buf[..n]);
        assert!(output.contains("\"error\""), "expected error in: {output}");
        assert!(output.contains("-32700"));
    }

    #[tokio::test]
    async fn io_loop_handles_invalid_request_envelope() {
        let nats = AdvancedMockNatsClient::new();
        let client = make_client(nats, MockJetStreamConsumerFactory::new());

        let (stdin_reader, mut stdin_writer) = tokio::io::duplex(4096);
        let (mut stdout_reader, stdout_writer) = tokio::io::duplex(4096);

        // Valid JSON but missing the required `method` field on InboundRequest.
        stdin_writer.write_all(b"{\"id\":1}\n").await.unwrap();
        drop(stdin_writer);

        let _ = run_io_loop(client, stdin_reader, stdout_writer, std::future::pending::<()>()).await;

        let mut buf = vec![0u8; 4096];
        let n = stdout_reader.read(&mut buf).await.unwrap();
        let output = String::from_utf8_lossy(&buf[..n]);
        assert!(output.contains("-32600"), "expected invalid request in: {output}");
    }

    #[tokio::test]
    async fn io_loop_shutdown_preempts_blocking_dispatch_acquire() {
        let nats = AdvancedMockNatsClient::new();
        let client = make_client(nats, MockJetStreamConsumerFactory::new());

        // Fill the loop with more requests than the semaphore allows, each one
        // landing on an unstubbed subject so the dispatcher never resolves.
        // The next `acquire_owned()` then sits indefinitely — shutdown must
        // preempt it, not hang behind it.
        let (stdin_reader, mut stdin_writer) = tokio::io::duplex(64 * 1024);
        let (_stdout_reader, stdout_writer) = tokio::io::duplex(64 * 1024);

        let line =
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tasks/get\",\"params\":{\"id\":\"never\",\"tenant\":\"\"}}\n";
        for _ in 0..(MAX_INFLIGHT_DISPATCH + 4) {
            stdin_writer.write_all(line).await.unwrap();
        }

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let shutdown = async move {
            let _ = shutdown_rx.await;
        };

        let handle = tokio::spawn(run_io_loop(client, stdin_reader, stdout_writer, shutdown));

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let _ = shutdown_tx.send(());

        let res = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        assert!(res.is_ok(), "io_loop did not exit on shutdown");
        drop(stdin_writer);
    }

    // AsyncWrite that fails on the N-th poll_write/poll_flush call. Used to
    // exercise the writer task's three I/O error branches (write payload,
    // write newline, flush) which are otherwise unreachable with duplex pipes.
    struct FailingWriter {
        writes_until_fail: std::sync::atomic::AtomicUsize,
        fail_on_flush: bool,
    }
    impl tokio::io::AsyncWrite for FailingWriter {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            let remaining = self.writes_until_fail.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            if remaining == 0 {
                std::task::Poll::Ready(Err(std::io::Error::other("write boom")))
            } else {
                std::task::Poll::Ready(Ok(buf.len()))
            }
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if self.fail_on_flush {
                std::task::Poll::Ready(Err(std::io::Error::other("flush boom")))
            } else {
                std::task::Poll::Ready(Ok(()))
            }
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    async fn run_with_failing_writer(writes_until_fail: usize, fail_on_flush: bool) -> std::io::Result<()> {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agents.bot.tasks.get", task_response("t-fw"));
        let client = make_client(nats, MockJetStreamConsumerFactory::new());

        let (stdin_reader, mut stdin_writer) = tokio::io::duplex(4096);
        let writer = FailingWriter {
            writes_until_fail: std::sync::atomic::AtomicUsize::new(writes_until_fail),
            fail_on_flush,
        };

        let req =
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tasks/get\",\"params\":{\"id\":\"t-fw\",\"tenant\":\"\"}}\n";
        stdin_writer.write_all(req).await.unwrap();
        drop(stdin_writer);

        run_io_loop(client, stdin_reader, writer, std::future::pending::<()>()).await
    }

    #[tokio::test]
    async fn writer_task_handles_payload_write_failure() {
        let res = run_with_failing_writer(0, false).await;
        assert!(res.is_err(), "writer failure must propagate as Err");
    }

    #[tokio::test]
    async fn writer_task_handles_newline_write_failure() {
        let res = run_with_failing_writer(1, false).await;
        assert!(res.is_err(), "writer failure must propagate as Err");
    }

    #[tokio::test]
    async fn writer_task_handles_flush_failure() {
        let res = run_with_failing_writer(usize::MAX, true).await;
        assert!(res.is_err(), "writer failure must propagate as Err");
    }

    // AsyncRead that surfaces an error on first poll_read. Covers the
    // `Err(e) => break` stdin-read branch.
    struct FailingReader;
    impl tokio::io::AsyncRead for FailingReader {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Err(std::io::Error::other("read boom")))
        }
    }

    #[tokio::test]
    async fn io_loop_exits_on_stdin_read_error() {
        let nats = AdvancedMockNatsClient::new();
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let (_stdout_reader, stdout_writer) = tokio::io::duplex(1024);
        let _ = run_io_loop(client, FailingReader, stdout_writer, std::future::pending::<()>()).await;
    }

    #[tokio::test]
    async fn failing_writer_shutdown_returns_ready_ok() {
        let mut w = FailingWriter {
            writes_until_fail: std::sync::atomic::AtomicUsize::new(usize::MAX),
            fail_on_flush: false,
        };
        // Exercise the success branch of poll_flush + poll_shutdown.
        w.flush().await.unwrap();
        w.shutdown().await.unwrap();
    }

    #[test]
    fn parse_inbound_routes_syntax_to_parse_error_and_shape_to_invalid_request() {
        assert_eq!(parse_inbound("not json").unwrap_err().error.code, -32700);
        assert_eq!(parse_inbound(r#"{"id":1}"#).unwrap_err().error.code, -32600);
        let (id, method, _) = parse_inbound(r#"{"jsonrpc":"2.0","id":7,"method":"tasks/get","params":{}}"#).unwrap();
        assert_eq!(id, RpcId::Number(7));
        assert_eq!(method, "tasks/get");
    }

    #[test]
    fn parse_inbound_preserves_id_on_envelope_failure() {
        let err = parse_inbound(r#"{"jsonrpc":"2.0","id":42}"#).unwrap_err();
        assert_eq!(err.id, RpcId::Number(42));
        let err = parse_inbound(r#"{"jsonrpc":"2.0","id":"corr-7"}"#).unwrap_err();
        assert_eq!(err.id, RpcId::String("corr-7".into()));
        let err = parse_inbound(r#"{"jsonrpc":"2.0"}"#).unwrap_err();
        assert_eq!(err.id, RpcId::Null);
        let err = parse_inbound(r#"{"jsonrpc":"2.0","id":[1,2,3]}"#).unwrap_err();
        assert_eq!(err.id, RpcId::Null);
    }

    #[test]
    fn parse_inbound_rejects_missing_or_wrong_jsonrpc_version() {
        // Missing `jsonrpc` field → -32600.
        let err = parse_inbound(r#"{"id":1,"method":"tasks/get","params":{}}"#).unwrap_err();
        assert_eq!(err.error.code, -32600);
        assert_eq!(err.id, RpcId::Number(1));
        // Wrong version → -32600.
        let err = parse_inbound(r#"{"jsonrpc":"1.0","id":2,"method":"tasks/get","params":{}}"#).unwrap_err();
        assert_eq!(err.error.code, -32600);
        assert_eq!(err.id, RpcId::Number(2));
        // Non-string version → -32600.
        let err = parse_inbound(r#"{"jsonrpc":2.0,"id":3}"#).unwrap_err();
        assert_eq!(err.error.code, -32600);
    }
}
