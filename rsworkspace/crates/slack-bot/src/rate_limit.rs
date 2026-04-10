use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Token-bucket rate limiter. Enforces a minimum interval between calls.
/// Thread-safe; cheap to clone (Arc-wrapped internally by callers).
pub struct RateLimiter {
    min_interval: Duration,
    last: Mutex<Option<Instant>>,
}

impl RateLimiter {
    /// Create a limiter that allows `rps` requests per second (must be > 0).
    pub fn new(rps: f32) -> Self {
        let secs = 1.0 / rps;
        let min_interval = Duration::from_secs_f32(secs);
        Self {
            min_interval,
            last: Mutex::new(None),
        }
    }

    /// Wait until the next slot is available, then claim it.
    pub async fn acquire(&self) {
        let sleep_for = {
            let mut guard = self.last.lock().unwrap();
            match *guard {
                None => {
                    *guard = Some(Instant::now());
                    None
                }
                Some(last_instant) => {
                    let elapsed = last_instant.elapsed();
                    if elapsed >= self.min_interval {
                        *guard = Some(Instant::now());
                        None
                    } else {
                        let sleep_for = self.min_interval - elapsed;
                        Some(sleep_for)
                    }
                }
            }
        };

        if let Some(duration) = sleep_for {
            tokio::time::sleep(duration).await;
            let mut guard = self.last.lock().unwrap();
            *guard = Some(Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sets_correct_interval() {
        let limiter = RateLimiter::new(2.0);
        let expected = Duration::from_millis(500);
        assert_eq!(limiter.min_interval, expected);
    }

    #[tokio::test]
    async fn acquire_does_not_block_on_first_call() {
        let limiter = RateLimiter::new(1.0);
        let start = tokio::time::Instant::now();
        limiter.acquire().await;
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(100),
            "First acquire took too long: {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn acquire_throttles_second_call() {
        // Use a very short interval (50ms = 20 rps) so the test is fast.
        let limiter = RateLimiter::new(20.0);
        let start = tokio::time::Instant::now();
        limiter.acquire().await; // first: instant
        limiter.acquire().await; // second: should wait ~50ms
        let elapsed = start.elapsed();
        let min_interval = Duration::from_millis(50);
        assert!(
            elapsed >= min_interval,
            "Two back-to-back acquires should take at least {min_interval:?}, but took {elapsed:?}"
        );
    }
}
