use std::sync::atomic::Ordering;

use trogon_sandbox_contract::{StdinChunk, StdinEof, StdinSeq, WriteStdinResponse};

use super::*;
use crate::constants::STDIN_OP_CANCELLED;
use crate::run_stdin_writer;
use crate::test_fixtures::{TestResult, stdin_write};

fn chunk(bytes: &[u8]) -> Result<StdinChunk, Box<dyn std::error::Error>> {
    Ok(StdinChunk::new(bytes.to_vec())?)
}

#[tokio::test(start_paused = true)]
async fn apply_without_a_writer_is_backpressure_and_the_write_is_cancelled() -> TestResult {
    let (handle, mut ops) = StdinHandle::channel();
    let result = handle.apply(stdin_write(b"x", false)?).await;
    assert_eq!(result, Err(StdinApplyError::Backpressure));

    let op = ops.recv().await.ok_or("no queued write")?;
    assert_eq!(op.phase.load(Ordering::Acquire), STDIN_OP_CANCELLED);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_full_queue_is_backpressure() -> TestResult {
    let (handle, _ops) = StdinHandle::channel();
    for _ in 0..crate::constants::STDIN_QUEUE_DEPTH {
        let _ = handle.apply(stdin_write(b"x", false)?).await;
    }
    let result = handle.apply(stdin_write(b"x", false)?).await;
    assert_eq!(result, Err(StdinApplyError::Backpressure));
    Ok(())
}

#[tokio::test]
async fn a_dropped_writer_is_broken_for_good() -> TestResult {
    let (handle, ops) = StdinHandle::channel();
    drop(ops);
    assert_eq!(
        handle.apply(stdin_write(b"x", false)?).await,
        Err(StdinApplyError::Broken)
    );
    let response = handle.deliver(StdinSeq::new(0), chunk(b"y")?, StdinEof::Keep).await;
    assert_eq!(response, WriteStdinResponse::StdinClosed);
    Ok(())
}

#[tokio::test]
async fn deliver_applies_in_order_and_reports_every_admission() -> TestResult {
    let (input, _output) = tokio::io::duplex(4096);
    let (handle, ops) = StdinHandle::channel();
    tokio::spawn(run_stdin_writer(Box::pin(input), ops));

    let written = handle.deliver(StdinSeq::new(0), chunk(b"a")?, StdinEof::Keep).await;
    assert_eq!(
        written,
        WriteStdinResponse::Written {
            next_seq: StdinSeq::new(1)
        }
    );
    let duplicate = handle.deliver(StdinSeq::new(0), chunk(b"a")?, StdinEof::Keep).await;
    assert_eq!(
        duplicate,
        WriteStdinResponse::Duplicate {
            next_seq: StdinSeq::new(1)
        }
    );
    let buffered = handle.deliver(StdinSeq::new(3), chunk(b"d")?, StdinEof::Keep).await;
    assert_eq!(
        buffered,
        WriteStdinResponse::Buffered {
            next_seq: StdinSeq::new(1)
        }
    );
    let far = handle.deliver(StdinSeq::new(1000), chunk(b"z")?, StdinEof::Keep).await;
    assert_eq!(
        far,
        WriteStdinResponse::OutOfWindow {
            next_seq: StdinSeq::new(1)
        }
    );
    let closing = handle.deliver(StdinSeq::new(1), chunk(b"b")?, StdinEof::Close).await;
    assert_eq!(
        closing,
        WriteStdinResponse::Written {
            next_seq: StdinSeq::new(2)
        }
    );
    let after = handle.deliver(StdinSeq::new(2), chunk(b"c")?, StdinEof::Keep).await;
    assert_eq!(after, WriteStdinResponse::StdinClosed);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn deliver_without_a_writer_answers_backpressure_with_the_same_sequence() -> TestResult {
    let (handle, _ops) = StdinHandle::channel();
    let response = handle.deliver(StdinSeq::new(0), chunk(b"a")?, StdinEof::Keep).await;
    assert_eq!(
        response,
        WriteStdinResponse::Backpressure {
            next_seq: StdinSeq::new(0)
        }
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_started_write_that_never_finishes_breaks_stdin() -> TestResult {
    let (handle, mut ops) = StdinHandle::channel();
    let taker = tokio::spawn(async move {
        if let Some(op) = ops.recv().await {
            op.phase.store(crate::constants::STDIN_OP_STARTED, Ordering::Release);
            // Hold the acknowledgement so the write never completes.
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            drop(op);
        }
    });
    let result = handle.apply(stdin_write(b"x", false)?).await;
    assert_eq!(result, Err(StdinApplyError::Broken));
    taker.abort();
    let response = handle.deliver(StdinSeq::new(0), chunk(b"y")?, StdinEof::Keep).await;
    assert_eq!(response, WriteStdinResponse::StdinClosed);
    Ok(())
}

#[tokio::test]
async fn a_writer_that_drops_the_acknowledgement_breaks_stdin() -> TestResult {
    let (handle, mut ops) = StdinHandle::channel();
    tokio::spawn(async move {
        if let Some(op) = ops.recv().await {
            drop(op.ack);
        }
    });
    assert_eq!(
        handle.apply(stdin_write(b"x", false)?).await,
        Err(StdinApplyError::Broken)
    );
    Ok(())
}
