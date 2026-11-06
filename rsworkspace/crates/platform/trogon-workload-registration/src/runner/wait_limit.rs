use std::future::Future;
use std::time::{Duration, Instant as StdInstant};

use tokio::time::Instant;

use super::runner_error::RunnerError;
use super::timeout_cause::TimeoutCause;

pub(super) struct WaitLimit {
    deadline: Instant,
    cause: TimeoutCause,
}

#[cfg(test)]
mod tests;

impl WaitLimit {
    const IO_TIMEOUT: Duration = Duration::from_secs(5);

    pub fn input(current: Option<StdInstant>) -> Self {
        Self::bounded(TimeoutCause::Frame, current.into_iter())
    }

    pub fn authority(deadline: StdInstant) -> Self {
        Self {
            deadline: deadline.into(),
            cause: TimeoutCause::Authority,
        }
    }

    pub fn effect(approval: Option<StdInstant>, current: Option<StdInstant>) -> Self {
        Self::bounded(TimeoutCause::Effect, approval.into_iter().chain(current))
    }

    fn bounded(cause: TimeoutCause, authority: impl Iterator<Item = StdInstant>) -> Self {
        let mut limit = Self {
            deadline: Instant::now() + Self::IO_TIMEOUT,
            cause,
        };
        for deadline in authority {
            let deadline = Instant::from_std(deadline);
            if deadline <= limit.deadline {
                limit = Self {
                    deadline,
                    cause: TimeoutCause::Authority,
                };
            }
        }
        limit
    }

    pub async fn run<T>(&self, future: impl Future<Output = Result<T, RunnerError>>) -> Result<T, RunnerError> {
        self.check()?;
        let value = tokio::time::timeout_at(self.deadline, future)
            .await
            .map_err(|_| self.cause.error())??;
        self.check()?;
        Ok(value)
    }

    fn check(&self) -> Result<(), RunnerError> {
        if Instant::now() >= self.deadline {
            Err(self.cause.error())
        } else {
            Ok(())
        }
    }
}
