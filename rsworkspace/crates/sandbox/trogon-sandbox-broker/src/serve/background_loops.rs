use std::time::Duration;

use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Starts the broker's periodic work on tracked tasks that stop once `cancel` fires.
#[derive(Clone)]
pub(super) struct BackgroundLoops {
    tasks: TaskTracker,
    cancel: CancellationToken,
}

impl BackgroundLoops {
    pub(super) fn new(tasks: TaskTracker, cancel: CancellationToken) -> Self {
        Self { tasks, cancel }
    }

    /// The token every loop watches.
    pub(super) fn cancel(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Runs `work` right away and then every `period`, skipping ticks missed while it ran.
    pub(super) fn every<F, Fut>(&self, period: Duration, mut work: F)
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let cancel = self.cancel.clone();
        self.tasks.spawn(async move {
            let mut interval = tokio::time::interval(period);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            while cancel.run_until_cancelled(interval.tick()).await.is_some() {
                work().await;
            }
        });
    }

    /// Runs a loop that watches [`BackgroundLoops::cancel`] itself.
    pub(super) fn spawn<Fut>(&self, work: Fut)
    where
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.tasks.spawn(work);
    }
}

#[cfg(test)]
mod tests;
