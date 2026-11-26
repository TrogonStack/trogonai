use std::time::Instant;

use trogon_sandbox_contract::{LifecycleEvent, SandboxPhase};

/// Tracks the wall time of a single phase. Finish it with [`PhaseTimer::complete`] or [`PhaseTimer::fail`].
#[derive(Debug)]
pub struct PhaseTimer {
    phase: SandboxPhase,
    detail: Option<String>,
    started: Instant,
}

impl PhaseTimer {
    /// Starts timing `phase`.
    pub fn start(phase: SandboxPhase) -> Self {
        Self::start_with_detail(phase, None)
    }

    /// Starts timing `phase`, carrying `detail` on every event the timer produces.
    pub fn start_with_detail(phase: SandboxPhase, detail: Option<String>) -> Self {
        Self {
            phase,
            detail,
            started: Instant::now(),
        }
    }

    /// The event announcing the phase began.
    pub fn phase_started(&self) -> LifecycleEvent {
        LifecycleEvent::PhaseStarted {
            phase: self.phase,
            detail: self.detail.clone(),
        }
    }

    /// The event reporting the phase finished, with its elapsed time.
    pub fn complete(self) -> LifecycleEvent {
        LifecycleEvent::PhaseCompleted {
            phase: self.phase,
            duration_ms: self.elapsed_ms(),
            detail: self.detail,
        }
    }

    /// The event reporting the phase failed with `error`, with its elapsed time.
    pub fn fail(self, error: impl Into<String>) -> LifecycleEvent {
        LifecycleEvent::PhaseFailed {
            phase: self.phase,
            duration_ms: self.elapsed_ms(),
            error: error.into(),
            detail: self.detail,
        }
    }

    fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
    }
}
