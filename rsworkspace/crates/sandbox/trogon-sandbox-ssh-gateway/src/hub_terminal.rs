use std::pin::Pin;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio::io::AsyncWrite;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{LeaseId, TerminalDimension};
use trogon_sandbox_protocol::ActivitySignal;

/// One open terminal of the hub: its guest channel and the clocks that keep it alive.
pub(crate) struct HubTerminal {
    pub(crate) lease_id: LeaseId,
    pub(crate) input: tokio::sync::Mutex<Pin<Box<dyn AsyncWrite + Send>>>,
    pub(crate) resize: mpsc::Sender<(TerminalDimension, TerminalDimension)>,
    /// Next output `seq`. Held across the publish so frames leave in order.
    pub(crate) seq: tokio::sync::Mutex<u64>,
    pub(crate) stop_reader: CancellationToken,
    pub(crate) close: CancellationToken,
    /// Paces the activity signal output sends, so a terminal that only prints (a build, `tail -f`) still counts
    /// toward the broker's idle timeout.
    pub(crate) activity: ActivitySignal,
    /// Last input or output, for the terminal's own idle timeout.
    last_io: Mutex<Instant>,
}

impl HubTerminal {
    pub(crate) fn new(
        lease_id: LeaseId,
        input: Pin<Box<dyn AsyncWrite + Send>>,
        resize: mpsc::Sender<(TerminalDimension, TerminalDimension)>,
        close: CancellationToken,
    ) -> Self {
        Self {
            lease_id,
            input: tokio::sync::Mutex::new(input),
            resize,
            seq: tokio::sync::Mutex::new(0),
            stop_reader: CancellationToken::new(),
            close,
            activity: ActivitySignal::default(),
            last_io: Mutex::new(Instant::now()),
        }
    }

    pub(crate) fn touch(&self) {
        *self.last_io.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Instant::now();
    }

    pub(crate) fn idle_for(&self) -> Duration {
        self.last_io
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .elapsed()
    }
}
