use std::fmt::Display;
use std::future::Future;

use trogon_sandbox_contract::SandboxPhase;

use crate::{LifecycleEmitter, PhaseTimer};

/// Runs `f`, emitting `PhaseStarted`, then `PhaseCompleted` or `PhaseFailed` with the error's message.
///
/// With no emitter the phase just runs. The result of `f` is returned unchanged.
pub async fn run_phase<Emitter, Work, Fut, Output, Error>(
    emitter: Option<&Emitter>,
    phase: SandboxPhase,
    detail: Option<String>,
    work: Work,
) -> Result<Output, Error>
where
    Emitter: LifecycleEmitter + ?Sized,
    Work: FnOnce() -> Fut,
    Fut: Future<Output = Result<Output, Error>>,
    Error: Display,
{
    let timer = PhaseTimer::start_with_detail(phase, detail);
    if let Some(emitter) = emitter {
        emitter.emit(timer.phase_started()).await;
    }
    match work().await {
        Ok(value) => {
            if let Some(emitter) = emitter {
                emitter.emit(timer.complete()).await;
            }
            Ok(value)
        }
        Err(error) => {
            if let Some(emitter) = emitter {
                emitter.emit(timer.fail(error.to_string())).await;
            }
            Err(error)
        }
    }
}
