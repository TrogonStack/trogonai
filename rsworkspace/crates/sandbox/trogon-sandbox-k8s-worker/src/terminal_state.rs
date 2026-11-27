use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_protocol::{ActivitySignal, TerminalReplayRing};

use crate::ExecInput;

pub(crate) struct TerminalState {
    pub(crate) lease_id: LeaseId,
    pub(crate) pod: String,
    pub(crate) input: tokio::sync::Mutex<ExecInput>,
    pub(crate) resize: tokio::sync::mpsc::Sender<(u16, u16)>,
    /// Next output `seq`. Held across the publish so frames leave in order.
    pub(crate) seq: tokio::sync::Mutex<u64>,
    /// The latest output, for a reader that reattaches. Only touched with
    /// `seq` held, so it always matches what was published.
    pub(crate) replay: Mutex<TerminalReplayRing>,
    pub(crate) stop_reader: CancellationToken,
    /// Paces the activity signal output sends, so a terminal that only
    /// prints (a build, `tail -f`) still counts toward the broker's idle
    /// timeout.
    pub(crate) activity: ActivitySignal,
    /// Last input or output, for the terminal's own idle timeout.
    pub(crate) last_io: Mutex<Instant>,
}

impl TerminalState {
    pub(crate) fn touch(&self) {
        *self.last_io.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

    pub(crate) fn idle_for(&self) -> Duration {
        self.last_io.lock().unwrap_or_else(|e| e.into_inner()).elapsed()
    }
}
