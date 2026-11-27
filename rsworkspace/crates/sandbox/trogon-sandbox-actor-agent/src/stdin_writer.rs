use std::sync::atomic::Ordering;

use tokio::io::AsyncWriteExt;
use tokio::process::ChildStdin;
use tokio::sync::{mpsc, oneshot};

use crate::constants::{PHASE_CANCELLED, PHASE_QUEUED, PHASE_STARTED};
use crate::stdin_op::StdinOp;

/// Applies queued writes to the process's stdin until stdin closes, a write fails, or the process finishes.
pub(crate) async fn run_stdin_writer(
    mut input: ChildStdin,
    mut ops: mpsc::Receiver<StdinOp>,
    mut finished: oneshot::Receiver<()>,
) {
    loop {
        let op = tokio::select! {
            received = ops.recv() => match received {
                Some(op) => op,
                None => return,
            },
            _ = &mut finished => return,
        };
        // A write the caller already gave up on must not reach the process, or a retry would apply it twice.
        let claimed = op
            .phase
            .compare_exchange(PHASE_QUEUED, PHASE_STARTED, Ordering::AcqRel, Ordering::Acquire);
        if claimed == Err(PHASE_CANCELLED) {
            continue;
        }
        let closes = op.write.eof.closes();
        let written = write_through(&mut input, op.write.data.as_slice(), closes).await;
        let delivered = written.is_ok();
        let _ = op.ack.send(delivered);
        if !delivered || closes {
            return;
        }
    }
}

async fn write_through(input: &mut ChildStdin, bytes: &[u8], close: bool) -> std::io::Result<()> {
    input.write_all(bytes).await?;
    input.flush().await?;
    if close {
        input.shutdown().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
