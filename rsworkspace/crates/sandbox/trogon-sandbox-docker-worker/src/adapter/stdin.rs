use super::*;

/// How a command's stdin is wired by [`DockerWorkerProvider::exec_collect`].
pub(super) enum ExecStdin {
    /// Closed immediately.
    None,
    /// Written once, then closed.
    Once(Vec<u8>),
    /// Fed by later `write_stdin` requests through the receiver until an
    /// `eof` write or until every [`StdinHandle`] is dropped.
    Interactive(mpsc::Receiver<StdinOp>),
    Supervised(Box<supervised_exec::Launch>),
}

pub(super) struct StdinOp {
    pub(super) write: StdinWrite,
    pub(super) ack: oneshot::Sender<bool>,
    pub(super) phase: Arc<AtomicU8>,
}

/// Why a stdin write did not reach the process.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum StdinApplyError {
    /// The write never started, so retrying the same `seq` is safe.
    Backpressure,
    /// Stdin can no longer be trusted: the write may be partly applied or
    /// the writer is gone, so no `seq` is retryable.
    Broken,
}

/// Client-facing side of an interactive exec's stdin. The sequencer lock is
/// held across a whole `write_stdin` request so writes to one execution are
/// applied strictly in order.
#[derive(Clone)]
pub(super) struct StdinHandle {
    pub(super) ops: mpsc::Sender<StdinOp>,
    pub(super) sequencer: Arc<Mutex<StdinSequencer>>,
    pub(super) broken: Arc<AtomicBool>,
}

impl StdinHandle {
    pub(super) fn channel() -> (Self, mpsc::Receiver<StdinOp>) {
        let (ops, receiver) = mpsc::channel(STDIN_QUEUE_DEPTH);
        let handle = Self {
            ops,
            sequencer: Arc::new(Mutex::new(StdinSequencer::new())),
            broken: Arc::new(AtomicBool::new(false)),
        };
        (handle, receiver)
    }

    pub(super) fn is_broken(&self) -> bool {
        self.broken.load(Ordering::Acquire)
    }

    /// Queues `write` and waits for the writer task to acknowledge it. When
    /// the deadline fires, a write the writer has not started is cancelled
    /// (retryable); one it already started leaves the outcome unknown, so
    /// stdin is marked broken instead of letting the caller retry it.
    pub(super) async fn apply(&self, write: StdinWrite) -> Result<(), StdinApplyError> {
        let (ack, mut acked) = oneshot::channel();
        let phase = Arc::new(AtomicU8::new(OP_QUEUED));
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
                Err(_) => match phase.compare_exchange(OP_QUEUED, OP_CANCELLED, Ordering::AcqRel, Ordering::Acquire) {
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
}

pub(super) async fn run_stdin_writer(
    mut input: std::pin::Pin<Box<dyn tokio::io::AsyncWrite + Send>>,
    mut ops: mpsc::Receiver<StdinOp>,
) {
    while let Some(op) = ops.recv().await {
        if op
            .phase
            .compare_exchange(OP_QUEUED, OP_STARTED, Ordering::AcqRel, Ordering::Acquire)
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
