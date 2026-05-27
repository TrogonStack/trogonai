use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::error::StsError;

const DEFAULT_FAILURE_THRESHOLD: u32 = 5;
const DEFAULT_COOLDOWN: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BreakerState {
    Closed,
    Open,
    HalfOpen,
}

struct BreakerInner {
    state: BreakerState,
    consecutive_failures: u32,
    opened_at: Option<Instant>,
}

impl BreakerInner {
    fn new() -> Self {
        Self {
            state: BreakerState::Closed,
            consecutive_failures: 0,
            opened_at: None,
        }
    }
}

#[derive(Clone)]
pub struct CircuitBreaker {
    inner: Arc<Mutex<BreakerInner>>,
    failure_threshold: u32,
    cooldown: Duration,
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new(DEFAULT_FAILURE_THRESHOLD, DEFAULT_COOLDOWN)
    }
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u32, cooldown: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BreakerInner::new())),
            failure_threshold,
            cooldown,
        }
    }

    pub async fn call<F, Fut, T>(&self, f: F) -> Result<T, StsError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, StsError>>,
    {
        {
            let mut guard = self.inner.lock().await;
            match guard.state {
                BreakerState::Open => {
                    let opened_at = guard
                        .opened_at
                        .expect("open breaker must have opened_at");
                    if opened_at.elapsed() >= self.cooldown {
                        guard.state = BreakerState::HalfOpen;
                    } else {
                        return Err(StsError::DependencyUnavailable(
                            "circuit breaker open".into(),
                        ));
                    }
                }
                BreakerState::Closed | BreakerState::HalfOpen => {}
            }
        }

        let result = f().await;

        let mut guard = self.inner.lock().await;
        match &result {
            Ok(_) => {
                guard.state = BreakerState::Closed;
                guard.consecutive_failures = 0;
                guard.opened_at = None;
            }
            Err(err) if is_dependency_failure(err) => match guard.state {
                BreakerState::HalfOpen => {
                    guard.state = BreakerState::Open;
                    guard.consecutive_failures = self.failure_threshold;
                    guard.opened_at = Some(Instant::now());
                }
                BreakerState::Closed => {
                    guard.consecutive_failures = guard.consecutive_failures.saturating_add(1);
                    if guard.consecutive_failures >= self.failure_threshold {
                        guard.state = BreakerState::Open;
                        guard.opened_at = Some(Instant::now());
                    }
                }
                BreakerState::Open => {}
            },
            Err(_) => {}
        }

        result
    }
}

pub fn is_dependency_failure(err: &StsError) -> bool {
    matches!(
        err,
        StsError::RegistryUnavailable(_) | StsError::SpiceDbUnavailable(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn trips_after_consecutive_failures() {
        let breaker = CircuitBreaker::new(3, Duration::from_secs(5));
        let err = StsError::RegistryUnavailable("down".into());

        for _ in 0..2 {
            let out = breaker.call(|| async { Err::<(), _>(err.clone()) }).await;
            assert!(matches!(out, Err(StsError::RegistryUnavailable(_))));
        }
        assert!(breaker.call(|| async { Ok(()) }).await.is_ok());

        for _ in 0..3 {
            let _ = breaker
                .call(|| async { Err::<(), _>(err.clone()) })
                .await;
        }
        assert!(matches!(
            breaker.call(|| async { Ok(()) }).await,
            Err(StsError::DependencyUnavailable(_))
        ));
    }

    #[tokio::test]
    async fn half_open_probe_success_closes_breaker() {
        let breaker = CircuitBreaker::new(1, Duration::from_millis(10));
        let err = StsError::RegistryUnavailable("down".into());
        let _ = breaker
            .call(|| async { Err::<(), _>(err.clone()) })
            .await;
        tokio::time::sleep(Duration::from_millis(15)).await;
        assert!(breaker.call(|| async { Ok(()) }).await.is_ok());
        assert!(breaker.call(|| async { Ok(()) }).await.is_ok());
    }

    #[tokio::test]
    async fn half_open_probe_failure_reopens_breaker() {
        let breaker = CircuitBreaker::new(1, Duration::from_millis(10));
        let err = StsError::RegistryUnavailable("down".into());
        let _ = breaker
            .call(|| async { Err::<(), _>(err.clone()) })
            .await;
        tokio::time::sleep(Duration::from_millis(15)).await;
        let _ = breaker
            .call(|| async { Err::<(), _>(err.clone()) })
            .await;
        assert!(matches!(
            breaker.call(|| async { Ok(()) }).await,
            Err(StsError::DependencyUnavailable(_))
        ));
    }
}
