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

pub async fn run_io_loop<N, J, R, W>(
    client: A2aClient<N, J>,
    stdin: R,
    mut stdout: W,
    shutdown: impl std::future::Future<Output = ()>,
) where
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

    let writer_task = tokio::spawn(async move {
        while let Some(frame) = frame_rx.recv().await {
            match serde_json::to_string(&frame) {
                Ok(json) => {
                    if let Err(e) = stdout.write_all(json.as_bytes()).await {
                        error!(error = %e, "stdout write failed");
                        return;
                    }
                    if let Err(e) = stdout.write_all(b"\n").await {
                        error!(error = %e, "stdout write failed");
                        return;
                    }
                    // Flush per frame so a piped parent doesn't deadlock
                    // waiting on a libc full-buffer that never drains until
                    // the next line on stdin closes the loop.
                    if let Err(e) = stdout.flush().await {
                        error!(error = %e, "stdout flush failed");
                        return;
                    }
                }
                Err(e) => {
                    error!(error = %e, "frame serialization failed");
                }
            }
        }
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

    'outer: loop {
        tokio::select! {
            _ = &mut shutdown => {
                debug!("io_loop received shutdown signal");
                shutdown_requested = true;
                break 'outer;
            }
            line = lines.next_line() => {
                match line {
                    Err(e) => {
                        error!(error = %e, "stdin read error");
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
                                // back-pressured; keep shutdown responsive.
                                tokio::select! {
                                    _ = &mut shutdown => {
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
                        // Poll shutdown alongside acquire so a saturated
                        // semaphore doesn't strand the signal.
                        let permit = tokio::select! {
                            _ = &mut shutdown => {
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
    while dispatch_tasks.join_next().await.is_some() {}
    drop(frame_tx);
    let _ = writer_task.await;
}

/// Split JSON-syntax failures (`-32700` Parse error) from envelope-shape
/// failures (`-32600` Invalid Request). JSON-RPC reserves `-32700` for actual
/// invalid JSON; structurally invalid requests are a different class.
fn parse_inbound(raw: &str) -> Result<(RpcId, String, serde_json::Value), OutboundError> {
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|e| {
        warn!(error = %e, "stdin line is not valid JSON");
        OutboundError::new(RpcId::Null, -32700, format!("parse error: {e}"))
    })?;
    let req: InboundRequest = serde_json::from_value(value).map_err(|e| {
        warn!(error = %e, "JSON-RPC envelope is invalid");
        OutboundError::new(RpcId::Null, -32600, format!("invalid request: {e}"))
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

        run_io_loop(client, stdin_reader, stdout_writer, std::future::pending::<()>()).await;
    }

    #[tokio::test]
    async fn io_loop_exits_on_shutdown_signal() {
        let nats = AdvancedMockNatsClient::new();
        let client = make_client(nats, MockJetStreamConsumerFactory::new());

        let (stdin_reader, _stdin_writer) = tokio::io::duplex(1024);
        let (_stdout_reader, stdout_writer) = tokio::io::duplex(1024);

        run_io_loop(client, stdin_reader, stdout_writer, std::future::ready(())).await;
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

        run_io_loop(client, stdin_reader, stdout_writer, std::future::pending::<()>()).await;

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

        run_io_loop(client, stdin_reader, stdout_writer, std::future::pending::<()>()).await;

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

        run_io_loop(client, stdin_reader, stdout_writer, std::future::pending::<()>()).await;

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

        run_io_loop(client, stdin_reader, stdout_writer, std::future::pending::<()>()).await;

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

    async fn run_with_failing_writer(writes_until_fail: usize, fail_on_flush: bool) {
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

        run_io_loop(client, stdin_reader, writer, std::future::pending::<()>()).await;
    }

    #[tokio::test]
    async fn writer_task_handles_payload_write_failure() {
        run_with_failing_writer(0, false).await;
    }

    #[tokio::test]
    async fn writer_task_handles_newline_write_failure() {
        run_with_failing_writer(1, false).await;
    }

    #[tokio::test]
    async fn writer_task_handles_flush_failure() {
        run_with_failing_writer(usize::MAX, true).await;
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
        run_io_loop(client, FailingReader, stdout_writer, std::future::pending::<()>()).await;
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
}
