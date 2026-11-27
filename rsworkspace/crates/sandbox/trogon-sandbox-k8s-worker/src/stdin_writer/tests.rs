use tokio::io::AsyncReadExt;

use super::*;
use crate::test_fixtures::{TestResult, stdin_write};
use crate::{StdinApplyError, StdinHandle};

#[tokio::test]
async fn writer_forwards_bytes_in_order_and_closes_on_eof() -> TestResult {
    let (input, mut output) = tokio::io::duplex(1024);
    let (handle, ops) = StdinHandle::channel();
    let writer = tokio::spawn(run_stdin_writer(Box::pin(input), ops));

    handle.apply(stdin_write(b"ab", false)?).await?;
    handle.apply(stdin_write(b"cd", true)?).await?;
    writer.await?;

    let mut got = Vec::new();
    output.read_to_end(&mut got).await?;
    assert_eq!(got, b"abcd");
    assert!(handle.apply(stdin_write(b"e", false)?).await.is_err());
    Ok(())
}

#[tokio::test]
async fn writer_closes_input_when_every_handle_is_dropped() -> TestResult {
    let (input, mut output) = tokio::io::duplex(1024);
    let (handle, ops) = StdinHandle::channel();
    let writer = tokio::spawn(run_stdin_writer(Box::pin(input), ops));

    handle.apply(stdin_write(b"x", false)?).await?;
    drop(handle);
    writer.await?;

    let mut got = Vec::new();
    output.read_to_end(&mut got).await?;
    assert_eq!(got, b"x");
    Ok(())
}

#[tokio::test]
async fn an_empty_write_only_closes_when_it_carries_eof() -> TestResult {
    let (input, mut output) = tokio::io::duplex(1024);
    let (handle, ops) = StdinHandle::channel();
    let writer = tokio::spawn(run_stdin_writer(Box::pin(input), ops));

    handle.apply(stdin_write(b"", false)?).await?;
    handle.apply(stdin_write(b"", true)?).await?;
    writer.await?;

    let mut got = Vec::new();
    output.read_to_end(&mut got).await?;
    assert!(got.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_failing_input_stops_the_writer_and_breaks_stdin() -> TestResult {
    let (input, output) = tokio::io::duplex(1);
    drop(output);
    let (handle, ops) = StdinHandle::channel();
    let writer = tokio::spawn(run_stdin_writer(Box::pin(input), ops));

    let result = handle.apply(stdin_write(b"x", false)?).await;
    writer.await?;
    assert_eq!(result, Err(StdinApplyError::Broken));
    Ok(())
}

#[tokio::test]
async fn a_cancelled_write_is_skipped_by_the_writer() -> TestResult {
    let (input, mut output) = tokio::io::duplex(1024);
    let (handle, ops) = StdinHandle::channel();
    let (ack, _acked) = tokio::sync::oneshot::channel();
    let phase = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(crate::constants::STDIN_OP_CANCELLED));
    let (sender, receiver) = tokio::sync::mpsc::channel(2);
    sender
        .send(StdinOp {
            write: stdin_write(b"skipped", false)?,
            ack,
            phase,
        })
        .await?;
    drop(sender);
    drop(handle);
    drop(ops);
    run_stdin_writer(Box::pin(input), receiver).await;
    let mut got = Vec::new();
    output.read_to_end(&mut got).await?;
    assert!(got.is_empty());
    Ok(())
}
