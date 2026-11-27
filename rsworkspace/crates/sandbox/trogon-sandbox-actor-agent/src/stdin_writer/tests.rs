use std::error::Error;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicU8;

use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use trogon_sandbox_contract::{StdinChunk, StdinEof};
use trogon_sandbox_protocol::StdinWrite;

use super::*;

fn spawn(program: &str) -> Result<Child, Box<dyn Error>> {
    Ok(Command::new(program)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?)
}

fn op(data: &[u8], eof: StdinEof, phase: u8) -> Result<(StdinOp, oneshot::Receiver<bool>), Box<dyn Error>> {
    let (ack, acked) = oneshot::channel();
    let op = StdinOp {
        write: StdinWrite {
            data: StdinChunk::new(data.to_vec())?,
            eof,
        },
        ack,
        phase: Arc::new(AtomicU8::new(phase)),
    };
    Ok((op, acked))
}

#[tokio::test]
async fn writes_reach_the_process_and_eof_closes_stdin() -> Result<(), Box<dyn Error>> {
    let mut child = spawn("cat")?;
    let input = child.stdin.take().ok_or("no stdin")?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let (ops, receiver) = mpsc::channel(4);
    let (_finished_tx, finished) = oneshot::channel();
    let writer = tokio::spawn(run_stdin_writer(input, receiver, finished));

    let (first, first_ack) = op(b"hello ", StdinEof::Keep, PHASE_QUEUED)?;
    ops.send(first).await?;
    assert!(first_ack.await?);
    let (second, second_ack) = op(b"world", StdinEof::Close, PHASE_QUEUED)?;
    ops.send(second).await?;
    assert!(second_ack.await?);
    writer.await?;

    let mut text = String::new();
    stdout.read_to_string(&mut text).await?;
    assert_eq!(text, "hello world");
    child.wait().await?;
    Ok(())
}

#[tokio::test]
async fn a_cancelled_write_is_skipped() -> Result<(), Box<dyn Error>> {
    let mut child = spawn("cat")?;
    let input = child.stdin.take().ok_or("no stdin")?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let (ops, receiver) = mpsc::channel(4);
    let (_finished_tx, finished) = oneshot::channel();
    let writer = tokio::spawn(run_stdin_writer(input, receiver, finished));

    let (cancelled, cancelled_ack) = op(b"skipped", StdinEof::Keep, PHASE_CANCELLED)?;
    ops.send(cancelled).await?;
    let (last, last_ack) = op(b"kept", StdinEof::Close, PHASE_QUEUED)?;
    ops.send(last).await?;
    assert!(last_ack.await?);
    assert!(cancelled_ack.await.is_err());
    writer.await?;

    let mut text = String::new();
    stdout.read_to_string(&mut text).await?;
    assert_eq!(text, "kept");
    child.wait().await?;
    Ok(())
}

#[tokio::test]
async fn a_write_to_a_dead_process_is_reported_failed() -> Result<(), Box<dyn Error>> {
    let mut child = spawn("true")?;
    let input = child.stdin.take().ok_or("no stdin")?;
    child.wait().await?;
    let (ops, receiver) = mpsc::channel(4);
    let (_finished_tx, finished) = oneshot::channel();
    let writer = tokio::spawn(run_stdin_writer(input, receiver, finished));

    let (dead, dead_ack) = op(b"x", StdinEof::Keep, PHASE_QUEUED)?;
    ops.send(dead).await?;
    assert!(!dead_ack.await?);
    writer.await?;
    Ok(())
}

#[tokio::test]
async fn the_writer_stops_when_the_queue_closes_or_the_process_finishes() -> Result<(), Box<dyn Error>> {
    for close_queue in [true, false] {
        let mut child = spawn("cat")?;
        let input = child.stdin.take().ok_or("no stdin")?;
        let (ops, receiver) = mpsc::channel::<StdinOp>(4);
        let (finished_tx, finished) = oneshot::channel();
        let writer = tokio::spawn(run_stdin_writer(input, receiver, finished));
        if close_queue {
            drop(ops);
        } else {
            drop(finished_tx);
        }
        writer.await?;
        child.kill().await?;
    }
    Ok(())
}
