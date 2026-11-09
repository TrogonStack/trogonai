use std::any::Any;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::time::Duration;

use futures_util::FutureExt;
use tokio::time::Instant;
use tracing::error;

#[derive(Clone, Copy, Debug)]
pub struct RestartBackoff {
    initial: Duration,
    max: Duration,
}

impl RestartBackoff {
    pub const fn new(initial: Duration, max: Duration) -> Self {
        Self { initial, max }
    }

    pub const fn initial(self) -> Duration {
        self.initial
    }

    pub const fn max(self) -> Duration {
        self.max
    }

    pub fn next(self, current: Duration) -> Duration {
        current.saturating_mul(2).min(self.max)
    }
}

pub async fn supervise<Make, Running, Output>(task: &'static str, backoff: RestartBackoff, mut make: Make) -> Output
where
    Make: FnMut() -> Running,
    Running: Future<Output = Output>,
{
    let mut delay = backoff.initial;
    loop {
        let started = Instant::now();
        match AssertUnwindSafe(make()).catch_unwind().await {
            Ok(output) => return output,
            Err(panic) => {
                error!(task, panic = panic_message(panic.as_ref()), restart_in = ?delay, "supervised task panicked");
            }
        }
        if started.elapsed() >= backoff.max {
            delay = backoff.initial;
        }
        tokio::time::sleep(delay).await;
        delay = backoff.next(delay);
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string panic payload")
}

#[cfg(test)]
mod tests;
