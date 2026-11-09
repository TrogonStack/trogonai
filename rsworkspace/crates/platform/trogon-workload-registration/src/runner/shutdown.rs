use tokio::signal::unix::{Signal, SignalKind, signal};

use super::runner_error::RunnerError;

pub(super) struct Shutdown {
    interrupt: Signal,
    terminate: Signal,
}

impl Shutdown {
    pub fn new() -> Result<Self, RunnerError> {
        Ok(Self {
            interrupt: signal(SignalKind::interrupt()).map_err(RunnerError::Signal)?,
            terminate: signal(SignalKind::terminate()).map_err(RunnerError::Signal)?,
        })
    }

    pub async fn wait(&mut self) {
        tokio::select! {
            _ = self.interrupt.recv() => {},
            _ = self.terminate.recv() => {},
        }
    }
}
