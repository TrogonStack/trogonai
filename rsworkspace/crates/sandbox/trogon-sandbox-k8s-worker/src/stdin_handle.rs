use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use tokio::sync::{Mutex, mpsc, oneshot};
use trogon_sandbox_contract::{StdinChunk, StdinEof, StdinSeq, WriteStdinResponse};
use trogon_sandbox_protocol::{StdinAdmit, StdinSequencer, StdinWrite};

use crate::constants::{
    STDIN_OP_CANCELLED, STDIN_OP_QUEUED, STDIN_QUEUE_DEPTH, STDIN_STARTED_WRITE_DEADLINE, STDIN_WRITE_DEADLINE,
};
use crate::{StdinApplyError, StdinOp};

/// Client-facing side of an interactive exec's stdin. The sequencer lock is
/// held across a whole `write_stdin` request so writes to one execution are
/// applied strictly in order.
#[derive(Clone)]
pub(crate) struct StdinHandle {
    ops: mpsc::Sender<StdinOp>,
    sequencer: Arc<Mutex<StdinSequencer>>,
    broken: Arc<AtomicBool>,
}

impl StdinHandle {
    pub(crate) fn channel() -> (Self, mpsc::Receiver<StdinOp>) {
        let (ops, receiver) = mpsc::channel(STDIN_QUEUE_DEPTH);
        let handle = Self {
            ops,
            sequencer: Arc::new(Mutex::new(StdinSequencer::new())),
            broken: Arc::new(AtomicBool::new(false)),
        };
        (handle, receiver)
    }

    fn is_broken(&self) -> bool {
        self.broken.load(Ordering::Acquire)
    }

    /// Queues `write` and waits for the writer task to acknowledge it. When
    /// the deadline fires, a write the writer has not started is cancelled
    /// (retryable); one it already started leaves the outcome unknown, so
    /// stdin is marked broken instead of letting the caller retry it.
    pub(crate) async fn apply(&self, write: StdinWrite) -> Result<(), StdinApplyError> {
        let (ack, mut acked) = oneshot::channel();
        let phase = Arc::new(AtomicU8::new(STDIN_OP_QUEUED));
        let op = StdinOp {
            write,
            ack,
            phase: Arc::clone(&phase),
        };
        let deadline = tokio::time::Instant::now() + STDIN_WRITE_DEADLINE;
        let result = match tokio::time::timeout_at(deadline, self.ops.send(op)).await {
            Err(_) => Err(StdinApplyError::Backpressure),
            Ok(Err(_)) => Err(StdinApplyError::Broken),
            Ok(Ok(())) => match tokio::time::timeout_at(deadline, &mut acked).await {
                Ok(Ok(true)) => Ok(()),
                Ok(_) => Err(StdinApplyError::Broken),
                Err(_) => match phase.compare_exchange(
                    STDIN_OP_QUEUED,
                    STDIN_OP_CANCELLED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => Err(StdinApplyError::Backpressure),
                    Err(_) => match tokio::time::timeout(STDIN_STARTED_WRITE_DEADLINE, &mut acked).await {
                        Ok(Ok(true)) => Ok(()),
                        _ => Err(StdinApplyError::Broken),
                    },
                },
            },
        };
        if result == Err(StdinApplyError::Broken) {
            self.broken.store(true, Ordering::Release);
        }
        result
    }

    /// Delivers one stdin write, applying writes at most once and in `seq`
    /// order via [`StdinSequencer`]. The sequencer only advances once the
    /// process accepted the bytes, so a write that timed out before it
    /// started can be retried with the same `seq`. A write that failed or
    /// timed out after it started leaves stdin closed for good, since its
    /// effect on the process is unknown.
    pub(crate) async fn deliver(&self, seq: StdinSeq, data: StdinChunk, eof: StdinEof) -> WriteStdinResponse {
        let mut sequencer = self.sequencer.lock().await;
        if self.is_broken() {
            return WriteStdinResponse::StdinClosed;
        }
        match sequencer.admit(seq, data, eof) {
            StdinAdmit::Duplicate => WriteStdinResponse::Duplicate {
                next_seq: sequencer.next_seq(),
            },
            StdinAdmit::Buffered => WriteStdinResponse::Buffered {
                next_seq: sequencer.next_seq(),
            },
            StdinAdmit::OutOfWindow => WriteStdinResponse::OutOfWindow {
                next_seq: sequencer.next_seq(),
            },
            StdinAdmit::Closed => WriteStdinResponse::StdinClosed,
            StdinAdmit::Apply(writes) => {
                let mut applied = 0;
                let mut failure = None;
                for write in writes {
                    match self.apply(write).await {
                        Ok(()) => applied += 1,
                        Err(error) => {
                            failure = Some(error);
                            break;
                        }
                    }
                }
                sequencer.commit(applied);
                match failure {
                    None => WriteStdinResponse::Written {
                        next_seq: sequencer.next_seq(),
                    },
                    Some(StdinApplyError::Backpressure) => WriteStdinResponse::Backpressure {
                        next_seq: sequencer.next_seq(),
                    },
                    Some(StdinApplyError::Broken) => WriteStdinResponse::StdinClosed,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
