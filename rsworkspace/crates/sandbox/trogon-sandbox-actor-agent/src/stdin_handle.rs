use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::time::{Instant, timeout, timeout_at};
use trogon_sandbox_protocol::{StdinSequencer, StdinWrite};

use crate::constants::{PHASE_CANCELLED, PHASE_QUEUED, STDIN_QUEUE_DEPTH};
use crate::stdin_apply_error::StdinApplyError;
use crate::stdin_deadlines::StdinDeadlines;
use crate::stdin_op::StdinOp;

/// The caller's side of an execution's stdin.
///
/// The sequencer lock is held across a whole write request so writes to one execution are applied strictly in order.
#[derive(Debug, Clone)]
pub(crate) struct StdinHandle {
    ops: mpsc::Sender<StdinOp>,
    pub(crate) sequencer: Arc<Mutex<StdinSequencer>>,
    broken: Arc<AtomicBool>,
    deadlines: StdinDeadlines,
}

impl StdinHandle {
    pub(crate) fn channel() -> (Self, mpsc::Receiver<StdinOp>) {
        Self::channel_with(STDIN_QUEUE_DEPTH, StdinDeadlines::default())
    }

    pub(crate) fn channel_with(depth: usize, deadlines: StdinDeadlines) -> (Self, mpsc::Receiver<StdinOp>) {
        let (ops, receiver) = mpsc::channel(depth);
        let handle = Self {
            ops,
            sequencer: Arc::new(Mutex::new(StdinSequencer::new())),
            broken: Arc::new(AtomicBool::new(false)),
            deadlines,
        };
        (handle, receiver)
    }

    pub(crate) fn is_broken(&self) -> bool {
        self.broken.load(Ordering::Acquire)
    }

    /// Queues `write` and waits for the writer task to acknowledge it.
    ///
    /// When the deadline fires, a write the writer has not started is cancelled (retryable); one it already started
    /// leaves the outcome unknown, so stdin is marked broken.
    pub(crate) async fn apply(&self, write: StdinWrite) -> Result<(), StdinApplyError> {
        let (ack, mut acked) = oneshot::channel();
        let phase = Arc::new(AtomicU8::new(PHASE_QUEUED));
        let op = StdinOp {
            write,
            ack,
            phase: Arc::clone(&phase),
        };
        let deadline = Instant::now() + self.deadlines.accept;
        let result = match timeout_at(deadline, self.ops.send(op)).await {
            Err(_) => Err(StdinApplyError::Backpressure),
            Ok(Err(_)) => Err(StdinApplyError::Broken),
            Ok(Ok(())) => match timeout_at(deadline, &mut acked).await {
                Ok(Ok(true)) => Ok(()),
                Ok(_) => Err(StdinApplyError::Broken),
                Err(_) => self.settle_late(&phase, &mut acked).await,
            },
        };
        if result == Err(StdinApplyError::Broken) {
            self.broken.store(true, Ordering::Release);
        }
        result
    }

    /// The writer did not answer in time: cancel the write if it has not started, otherwise give it one more window.
    async fn settle_late(&self, phase: &AtomicU8, acked: &mut oneshot::Receiver<bool>) -> Result<(), StdinApplyError> {
        match phase.compare_exchange(PHASE_QUEUED, PHASE_CANCELLED, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => Err(StdinApplyError::Backpressure),
            Err(_) => match timeout(self.deadlines.started, acked).await {
                Ok(Ok(true)) => Ok(()),
                _ => Err(StdinApplyError::Broken),
            },
        }
    }
}

#[cfg(test)]
mod tests;
