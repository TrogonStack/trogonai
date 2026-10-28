use super::*;
use a2a_nats::client::A2aClient;
use a2a_nats::{A2aAgentId, A2aPrefix};
use bytes::Bytes;
use jsonrpc_nats::{Message as JrpcMessage, ResponseId, encode};
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use trogon_nats::AdvancedMockNatsClient;
use trogon_nats::jetstream::mocks::MockJetStreamConsumerFactory;

mod failure_tests;

fn make_client<N: RequestClient>(
    nats: N,
    js: MockJetStreamConsumerFactory,
) -> A2aClient<N, MockJetStreamConsumerFactory> {
    let prefix = A2aPrefix::new("a2a").unwrap();
    let agent_id = A2aAgentId::new("bot").unwrap();
    A2aClient::new(prefix, agent_id, nats, js)
}

#[derive(Clone)]
struct PendingRequests {
    started: tokio::sync::mpsc::UnboundedSender<()>,
}

impl RequestClient for PendingRequests {
    type RequestError = std::convert::Infallible;

    async fn request_with_headers<S: async_nats::subject::ToSubject + Send>(
        &self,
        _subject: S,
        _headers: async_nats::HeaderMap,
        _payload: Bytes,
    ) -> Result<async_nats::Message, Self::RequestError> {
        self.started.send(()).expect("request observer remains alive");
        std::future::pending().await
    }
}

fn task_response(task_id: &str) -> (async_nats::HeaderMap, Bytes) {
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
    let encoded = encode(&JrpcMessage::Success {
        id: ResponseId::String("any".into()),
        result: serde_json::json!(task),
    })
    .unwrap();
    (encoded.headers, encoded.body)
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
    let (headers, body) = task_response("t-blank");
    nats.set_response_wire("a2a.v1.agents.bot.tasks.get", headers, body);
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
    let (headers, body) = task_response("t1");
    nats.set_response_wire("a2a.v1.agents.bot.tasks.get", headers, body);
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

    // Valid JSON but missing the `method` field a JSON-RPC request requires.
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
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let nats = PendingRequests { started: started_tx };
    let client = make_client(nats, MockJetStreamConsumerFactory::new());

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

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        for _ in 0..MAX_INFLIGHT_DISPATCH {
            assert_eq!(started_rx.recv().await, Some(()));
        }
    })
    .await
    .expect("dispatch capacity must be occupied before shutdown");
    assert!(matches!(
        started_rx.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    shutdown_tx.send(()).expect("loop is waiting for shutdown");
    tokio::time::timeout(std::time::Duration::from_secs(2), handle)
        .await
        .expect("saturated loop must respond to shutdown")
        .expect("loop must not panic")
        .expect("controlled shutdown succeeds");
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
    let (headers, body) = task_response("t-fw");
    nats.set_response_wire("a2a.v1.agents.bot.tasks.get", headers, body);
    let client = make_client(nats, MockJetStreamConsumerFactory::new());

    let (stdin_reader, mut stdin_writer) = tokio::io::duplex(4096);
    let writer = FailingWriter {
        writes_until_fail: std::sync::atomic::AtomicUsize::new(writes_until_fail),
        fail_on_flush,
    };

    let req = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tasks/get\",\"params\":{\"id\":\"t-fw\",\"tenant\":\"\"}}\n";
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
