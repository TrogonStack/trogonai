use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::oneshot;

use super::*;

const REQUEST: &[u8] =
    b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tasks/get\",\"params\":{\"id\":\"task\",\"tenant\":\"\"}}\n";

struct ObservedLines {
    lines: VecDeque<Vec<u8>>,
    observed: mpsc::UnboundedSender<()>,
}

impl AsyncRead for ObservedLines {
    fn poll_read(mut self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let Some(line) = self.lines.pop_front() else {
            return Poll::Pending;
        };
        buf.put_slice(&line);
        self.observed.send(()).expect("line observer remains alive");
        Poll::Ready(Ok(()))
    }
}

enum WriterFailure {
    BrokenPipe,
    Panic,
}

struct GatedWriter(oneshot::Receiver<WriterFailure>);

impl AsyncWrite for GatedWriter {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, _buf: &[u8]) -> Poll<std::io::Result<usize>> {
        match std::future::Future::poll(Pin::new(&mut self.0), cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(WriterFailure::BrokenPipe)) => {
                Poll::Ready(Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe)))
            }
            Poll::Ready(Ok(WriterFailure::Panic)) => panic!("writer failed while emitting a reply"),
            Poll::Ready(Err(error)) => panic!("writer controller disappeared: {error}"),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

async fn wait_for_lines(observed: &mut mpsc::UnboundedReceiver<()>, count: usize) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        for _ in 0..count {
            assert_eq!(observed.recv().await, Some(()));
        }
    })
    .await
    .expect("reader reaches the blocked operation");
}

async fn finish(handle: tokio::task::JoinHandle<std::io::Result<()>>) -> std::io::Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .expect("IO loop responds without waiting for stdin EOF")
        .expect("IO loop does not panic")
}

#[tokio::test]
async fn writer_failure_stops_reading_open_stdin() {
    for failure in [WriterFailure::BrokenPipe, WriterFailure::Panic] {
        let (observed, mut reads) = mpsc::unbounded_channel();
        let reader = ObservedLines {
            lines: VecDeque::from([b"invalid\n".to_vec()]),
            observed,
        };
        let (fail, writer) = oneshot::channel();
        let client = make_client(AdvancedMockNatsClient::new(), MockJetStreamConsumerFactory::new());
        let handle = tokio::spawn(run_io_loop(client, reader, GatedWriter(writer), std::future::pending()));
        wait_for_lines(&mut reads, 1).await;
        let expected_pipe = matches!(failure, WriterFailure::BrokenPipe);
        assert!(fail.send(failure).is_ok());
        let error = finish(handle).await.expect_err("writer failure propagates");
        if expected_pipe {
            assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        } else {
            assert!(
                error
                    .get_ref()
                    .is_some_and(|source| source.is::<tokio::task::JoinError>())
            );
        }
    }
}

#[tokio::test]
async fn blocked_parse_error_queue_remains_responsive_to_shutdown_and_writer_failure() {
    for shutdown_requested in [true, false] {
        let count = CHANNEL_CAP + 2;
        let (observed, mut reads) = mpsc::unbounded_channel();
        let reader = ObservedLines {
            lines: (0..count).map(|_| b"invalid\n".to_vec()).collect(),
            observed,
        };
        let (fail, writer) = oneshot::channel();
        let (stop, shutdown) = oneshot::channel();
        let client = make_client(AdvancedMockNatsClient::new(), MockJetStreamConsumerFactory::new());
        let handle = tokio::spawn(run_io_loop(client, reader, GatedWriter(writer), async {
            shutdown.await.expect("shutdown controller remains alive");
        }));
        wait_for_lines(&mut reads, count).await;
        if shutdown_requested {
            stop.send(()).expect("loop remains active");
        }
        assert!(fail.send(WriterFailure::BrokenPipe).is_ok());
        assert_eq!(
            finish(handle)
                .await
                .expect_err("writer failure wins during teardown")
                .kind(),
            std::io::ErrorKind::BrokenPipe
        );
    }
}

#[tokio::test]
async fn writer_failure_interrupts_a_saturated_dispatch_set() {
    let (started, mut requests) = mpsc::unbounded_channel();
    let (observed, mut reads) = mpsc::unbounded_channel();
    let mut lines = VecDeque::from([b"invalid\n".to_vec()]);
    lines.extend((0..=MAX_INFLIGHT_DISPATCH).map(|_| REQUEST.to_vec()));
    let reader = ObservedLines { lines, observed };
    let (fail, writer) = oneshot::channel();
    let client = make_client(PendingRequests { started }, MockJetStreamConsumerFactory::new());
    let handle = tokio::spawn(run_io_loop(client, reader, GatedWriter(writer), std::future::pending()));
    wait_for_lines(&mut reads, MAX_INFLIGHT_DISPATCH + 2).await;
    wait_for_lines(&mut requests, MAX_INFLIGHT_DISPATCH).await;
    assert!(fail.send(WriterFailure::BrokenPipe).is_ok());
    assert_eq!(
        finish(handle)
            .await
            .expect_err("writer failure cancels pending requests")
            .kind(),
        std::io::ErrorKind::BrokenPipe
    );
    assert_eq!(requests.recv().await, None);
}

#[derive(Clone)]
struct PanickingRequests;

impl RequestClient for PanickingRequests {
    type RequestError = std::convert::Infallible;

    async fn request_with_headers<S: async_nats::subject::ToSubject + Send>(
        &self,
        _subject: S,
        _headers: async_nats::HeaderMap,
        _payload: Bytes,
    ) -> Result<async_nats::Message, Self::RequestError> {
        panic!("request transport failed while producing a reply");
    }
}

#[tokio::test]
async fn dispatch_panics_propagate_after_eof_with_and_without_capacity_reuse() {
    for count in [1, MAX_INFLIGHT_DISPATCH + 1] {
        let (stdin, mut input) = tokio::io::duplex(64 * 1024);
        for _ in 0..count {
            input.write_all(REQUEST).await.unwrap();
        }
        drop(input);
        let client = make_client(PanickingRequests, MockJetStreamConsumerFactory::new());
        let error = run_io_loop(client, stdin, tokio::io::sink(), std::future::pending())
            .await
            .expect_err("a transport panic cannot count as a delivered reply");
        assert!(
            error
                .get_ref()
                .is_some_and(|source| source.is::<tokio::task::JoinError>())
        );
    }
}

#[tokio::test]
async fn completed_dispatches_free_capacity_and_every_reply_drains_on_eof() {
    let count = MAX_INFLIGHT_DISPATCH + 5;
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = task_response("capacity-reused");
    nats.set_response_wire("a2a.v1.agents.bot.tasks.get", headers, body);
    let client = make_client(nats, MockJetStreamConsumerFactory::new());
    let (stdin, mut input) = tokio::io::duplex(64 * 1024);
    let (mut output, writer) = tokio::io::duplex(64 * 1024);
    for _ in 0..count {
        input.write_all(REQUEST).await.unwrap();
    }
    drop(input);
    run_io_loop(client, stdin, writer, std::future::pending())
        .await
        .unwrap();
    let mut replies = String::new();
    output.read_to_string(&mut replies).await.unwrap();
    assert_eq!(replies.lines().count(), count);
    for reply in replies.lines() {
        let reply: serde_json::Value = serde_json::from_str(reply).unwrap();
        assert_eq!(reply["id"], 1);
        assert_eq!(reply["result"]["id"], "capacity-reused");
    }
}

#[tokio::test]
async fn invalid_raw_frame_stops_the_writer_before_emitting_a_partial_reply() {
    let (frames, incoming) = mpsc::channel(1);
    frames
        .send(OutboundFrame::RawBody(Bytes::from_static(b"{")))
        .await
        .unwrap();
    drop(frames);
    let (mut output, writer) = tokio::io::duplex(64);
    let error = write_frames(writer, incoming)
        .await
        .expect_err("invalid frame fails before writing");
    assert!(error.get_ref().is_some_and(|source| source.is::<serde_json::Error>()));
    let mut written = Vec::new();
    output.read_to_end(&mut written).await.unwrap();
    assert!(written.is_empty());
}
