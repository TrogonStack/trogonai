use std::pin::Pin;
use std::sync::atomic::Ordering;

use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::StdinOp;
use crate::constants::{STDIN_OP_QUEUED, STDIN_OP_STARTED};

/// Applies queued writes to `input` in order until stdin is closed or every handle is gone.
pub(crate) async fn run_stdin_writer(mut input: Pin<Box<dyn AsyncWrite + Send>>, mut ops: mpsc::Receiver<StdinOp>) {
    while let Some(op) = ops.recv().await {
        if op
            .phase
            .compare_exchange(STDIN_OP_QUEUED, STDIN_OP_STARTED, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            continue;
        }
        let bytes = op.write.data.as_slice();
        let written = async {
            if !bytes.is_empty() {
                input.write_all(bytes).await?;
                input.flush().await?;
            }
            if op.write.eof.closes() {
                input.shutdown().await?;
            }
            Ok::<(), std::io::Error>(())
        }
        .await;
        let ok = written.is_ok();
        let _ = op.ack.send(ok);
        if !ok || op.write.eof.closes() {
            return;
        }
    }
    let _ = input.shutdown().await;
}

#[cfg(test)]
mod tests;
