mod support;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use support::{TestResult, lock};
use tokio::time::Instant;
use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{StdinChunk, StdinEof, StdinSeq, WriteStdinResponse};
use trogon_sandbox_lsp::{BackpressurePolicy, LspError, StdinSink, StdinWriter};

type Call = (u64, usize, bool);
type Script = VecDeque<Result<WriteStdinResponse, ClientError>>;

#[derive(Clone, Default)]
struct ScriptedSink {
    script: Arc<Mutex<Script>>,
    calls: Arc<Mutex<Vec<Call>>>,
}

impl ScriptedSink {
    fn push(&self, response: Result<WriteStdinResponse, ClientError>) -> &Self {
        lock(&self.script).push_back(response);
        self
    }

    fn calls(&self) -> Vec<Call> {
        lock(&self.calls).clone()
    }
}

#[async_trait]
impl StdinSink for ScriptedSink {
    async fn send(&self, seq: StdinSeq, data: StdinChunk, eof: StdinEof) -> Result<WriteStdinResponse, ClientError> {
        lock(&self.calls).push((seq.get(), data.as_slice().len(), eof.closes()));
        lock(&self.script)
            .pop_front()
            .unwrap_or(Ok(WriteStdinResponse::Written {
                next_seq: StdinSeq::new(seq.get() + 1),
            }))
    }
}

fn written(next: u64) -> WriteStdinResponse {
    WriteStdinResponse::Written {
        next_seq: StdinSeq::new(next),
    }
}

fn backpressure(next: u64) -> WriteStdinResponse {
    WriteStdinResponse::Backpressure {
        next_seq: StdinSeq::new(next),
    }
}

fn timeout() -> ClientError {
    ClientError::Timeout(Duration::from_secs(1))
}

#[tokio::test]
async fn numbers_writes_from_zero() -> TestResult {
    let sink = ScriptedSink::default();
    let writer = StdinWriter::new(sink.clone());
    writer.write(b"ab", StdinEof::Keep).await?;
    writer.write(b"c", StdinEof::Keep).await?;
    assert_eq!(sink.calls(), vec![(0, 2, false), (1, 1, false)]);
    Ok(())
}

#[tokio::test]
async fn an_empty_write_without_eof_sends_nothing() -> TestResult {
    let sink = ScriptedSink::default();
    let writer = StdinWriter::new(sink.clone());
    writer.write(b"", StdinEof::Keep).await?;
    assert!(sink.calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn splits_large_writes_and_marks_eof_on_the_last_chunk() -> TestResult {
    let sink = ScriptedSink::default();
    let writer = StdinWriter::new(sink.clone());
    writer
        .write(&vec![0u8; StdinChunk::MAX_LEN + 10], StdinEof::Close)
        .await?;
    assert_eq!(sink.calls(), vec![(0, StdinChunk::MAX_LEN, false), (1, 10, true)]);
    let err = writer
        .write(b"x", StdinEof::Keep)
        .await
        .err()
        .ok_or("write after eof")?;
    assert!(matches!(err, LspError::Stdin(m) if m.contains("closed")));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn retries_backpressure_with_the_same_seq_and_doubling_delay() -> TestResult {
    let sink = ScriptedSink::default();
    sink.push(Ok(backpressure(0)))
        .push(Ok(backpressure(0)))
        .push(Ok(backpressure(0)))
        .push(Ok(written(1)));
    let writer = StdinWriter::new(sink.clone());
    let started = Instant::now();
    writer.write(b"hi", StdinEof::Keep).await?;
    assert_eq!(sink.calls(), vec![(0, 2, false); 4]);
    assert_eq!(started.elapsed(), Duration::from_millis(50 + 100 + 200));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn caps_the_backoff_delay() -> TestResult {
    let sink = ScriptedSink::default();
    for _ in 0..6 {
        sink.push(Ok(backpressure(0)));
    }
    sink.push(Ok(written(1)));
    let writer = StdinWriter::new(sink.clone());
    let started = Instant::now();
    writer.write(b"hi", StdinEof::Keep).await?;
    // 50 + 100 + 200 + 400 + 800 + 1000
    assert_eq!(started.elapsed(), Duration::from_millis(2550));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn gives_up_when_backpressure_outlasts_the_budget() -> TestResult {
    let sink = ScriptedSink::default();
    for _ in 0..1000 {
        sink.push(Ok(backpressure(0)));
    }
    let writer = StdinWriter::new(sink.clone());
    let started = Instant::now();
    let err = writer.write(b"hi", StdinEof::Keep).await.err().ok_or("no failure")?;
    assert!(matches!(&err, LspError::Stdin(m) if m.contains("backpressured at seq 0")));
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_secs(30));
    assert!(elapsed < Duration::from_secs(32));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_custom_policy_sets_the_budget() -> TestResult {
    let sink = ScriptedSink::default();
    for _ in 0..100 {
        sink.push(Ok(backpressure(0)));
    }
    let policy = BackpressurePolicy {
        initial_delay: Duration::from_millis(10),
        max_delay: Duration::from_millis(10),
        budget: Duration::from_millis(35),
    };
    let writer = StdinWriter::with_policy(sink.clone(), policy);
    assert!(writer.write(b"hi", StdinEof::Keep).await.is_err());
    assert_eq!(sink.calls().len(), 5);
    Ok(())
}

#[tokio::test]
async fn duplicate_counts_as_success() -> TestResult {
    let sink = ScriptedSink::default();
    sink.push(Ok(WriteStdinResponse::Duplicate {
        next_seq: StdinSeq::new(1),
    }));
    let writer = StdinWriter::new(sink.clone());
    writer.write(b"hi", StdinEof::Keep).await?;
    assert_eq!(sink.calls().len(), 1);
    Ok(())
}

#[tokio::test]
async fn out_of_window_resends_from_the_servers_next_seq() -> TestResult {
    let sink = ScriptedSink::default();
    sink.push(Ok(WriteStdinResponse::OutOfWindow {
        next_seq: StdinSeq::new(2),
    }));
    let writer = StdinWriter::new(sink.clone());
    writer
        .write(&vec![0u8; StdinChunk::MAX_LEN * 2 + 1], StdinEof::Keep)
        .await?;
    let seqs: Vec<u64> = sink.calls().iter().map(|c| c.0).collect();
    assert_eq!(seqs, vec![0, 2]);
    Ok(())
}

#[tokio::test]
async fn a_server_that_expects_an_older_seq_is_an_error() -> TestResult {
    let sink = ScriptedSink::default();
    sink.push(Ok(written(1)));
    sink.push(Ok(WriteStdinResponse::Buffered {
        next_seq: StdinSeq::new(0),
    }));
    let writer = StdinWriter::new(sink.clone());
    writer.write(b"a", StdinEof::Keep).await?;
    let err = writer.write(b"b", StdinEof::Keep).await.err().ok_or("no failure")?;
    assert!(matches!(err, LspError::Stdin(m) if m.contains("already acknowledged")));
    Ok(())
}

#[tokio::test]
async fn already_completed_on_eof_counts_as_success() -> TestResult {
    let sink = ScriptedSink::default();
    sink.push(Ok(WriteStdinResponse::AlreadyCompleted));
    let writer = StdinWriter::new(sink.clone());
    writer.close().await?;
    writer.close().await?;
    assert_eq!(sink.calls(), vec![(0, 0, true)]);
    Ok(())
}

#[tokio::test]
async fn already_completed_on_data_is_a_failure() {
    let sink = ScriptedSink::default();
    sink.push(Ok(WriteStdinResponse::AlreadyCompleted));
    let writer = StdinWriter::new(sink.clone());
    assert!(writer.write(b"x", StdinEof::Keep).await.is_err());
    assert!(writer.write(b"y", StdinEof::Keep).await.is_err());
    assert!(writer.close().await.is_err());
    assert_eq!(sink.calls().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn retries_transport_failures_then_gives_up() -> TestResult {
    let sink = ScriptedSink::default();
    sink.push(Err(timeout())).push(Ok(written(1)));
    let writer = StdinWriter::new(sink.clone());
    writer.write(b"x", StdinEof::Keep).await?;
    assert_eq!(sink.calls(), vec![(0, 1, false); 2]);

    let failing = ScriptedSink::default();
    for _ in 0..4 {
        failing.push(Err(timeout()));
    }
    let writer = StdinWriter::new(failing.clone());
    let err = writer.write(b"x", StdinEof::Keep).await.err().ok_or("no failure")?;
    assert!(matches!(err, LspError::Transport(_)));
    assert_eq!(failing.calls().len(), 4);
    Ok(())
}

#[tokio::test]
async fn a_failure_that_a_retry_cannot_fix_is_reported_at_once() -> TestResult {
    let sink = ScriptedSink::default();
    sink.push(Err(ClientError::FileNotFound("x".into())));
    let writer = StdinWriter::new(sink.clone());
    let err = writer.write(b"x", StdinEof::Keep).await.err().ok_or("no failure")?;
    assert!(matches!(err, LspError::Transport(_)));
    assert_eq!(sink.calls().len(), 1);
    Ok(())
}

#[tokio::test]
async fn stalled_buffered_replies_fail_after_a_few_resends() -> TestResult {
    let sink = ScriptedSink::default();
    for _ in 0..10 {
        sink.push(Ok(WriteStdinResponse::Buffered {
            next_seq: StdinSeq::new(0),
        }));
    }
    let writer = StdinWriter::new(sink.clone());
    let err = writer.write(b"x", StdinEof::Keep).await.err().ok_or("no failure")?;
    assert!(matches!(err, LspError::Stdin(m) if m.contains("no progress")));
    assert_eq!(sink.calls().len(), 4);
    Ok(())
}
