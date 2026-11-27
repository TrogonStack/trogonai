use std::error::Error;
use std::sync::atomic::Ordering;
use std::time::Duration;

use trogon_sandbox_contract::{StdinChunk, StdinEof};

use super::*;
use crate::constants::PHASE_STARTED;

fn write() -> Result<StdinWrite, Box<dyn Error>> {
    Ok(StdinWrite {
        data: StdinChunk::new(b"x".to_vec())?,
        eof: StdinEof::Keep,
    })
}

fn quick() -> StdinDeadlines {
    StdinDeadlines {
        accept: Duration::from_millis(50),
        started: Duration::from_millis(50),
    }
}

#[tokio::test]
async fn an_acknowledged_write_is_applied() -> Result<(), Box<dyn Error>> {
    let (handle, mut ops) = StdinHandle::channel();
    tokio::spawn(async move {
        if let Some(op) = ops.recv().await {
            let _ = op.ack.send(true);
        }
    });
    assert_eq!(handle.apply(write()?).await, Ok(()));
    assert!(!handle.is_broken());
    Ok(())
}

#[tokio::test]
async fn a_failed_acknowledgement_breaks_stdin() -> Result<(), Box<dyn Error>> {
    let (handle, mut ops) = StdinHandle::channel();
    tokio::spawn(async move {
        if let Some(op) = ops.recv().await {
            let _ = op.ack.send(false);
        }
    });
    assert_eq!(handle.apply(write()?).await, Err(StdinApplyError::Broken));
    assert!(handle.is_broken());
    Ok(())
}

#[tokio::test]
async fn a_writer_that_is_gone_breaks_stdin() -> Result<(), Box<dyn Error>> {
    let (handle, ops) = StdinHandle::channel();
    drop(ops);
    assert_eq!(handle.apply(write()?).await, Err(StdinApplyError::Broken));
    Ok(())
}

#[tokio::test]
async fn a_write_nobody_picks_up_is_cancelled_and_retryable() -> Result<(), Box<dyn Error>> {
    let (handle, _ops) = StdinHandle::channel_with(4, quick());
    assert_eq!(handle.apply(write()?).await, Err(StdinApplyError::Backpressure));
    assert!(!handle.is_broken());
    Ok(())
}

#[tokio::test]
async fn a_full_queue_is_backpressure() -> Result<(), Box<dyn Error>> {
    let (handle, _ops) = StdinHandle::channel_with(1, quick());
    let first = handle.clone();
    let queued = write()?;
    // The first write occupies the only slot and is never acknowledged, so the second cannot even be queued.
    tokio::spawn(async move { first.apply(queued).await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(handle.apply(write()?).await, Err(StdinApplyError::Backpressure));
    Ok(())
}

#[tokio::test]
async fn a_started_write_that_finishes_late_is_still_applied() -> Result<(), Box<dyn Error>> {
    let (handle, mut ops) = StdinHandle::channel_with(4, quick());
    tokio::spawn(async move {
        if let Some(op) = ops.recv().await {
            op.phase.store(PHASE_STARTED, Ordering::Release);
            tokio::time::sleep(Duration::from_millis(80)).await;
            let _ = op.ack.send(true);
        }
    });
    assert_eq!(handle.apply(write()?).await, Ok(()));
    Ok(())
}

#[tokio::test]
async fn a_started_write_that_never_finishes_breaks_stdin() -> Result<(), Box<dyn Error>> {
    let (handle, mut ops) = StdinHandle::channel_with(4, quick());
    tokio::spawn(async move {
        if let Some(op) = ops.recv().await {
            op.phase.store(PHASE_STARTED, Ordering::Release);
            tokio::time::sleep(Duration::from_secs(5)).await;
            drop(op);
        }
    });
    assert_eq!(handle.apply(write()?).await, Err(StdinApplyError::Broken));
    assert!(handle.is_broken());
    Ok(())
}
