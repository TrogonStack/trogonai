use std::time::Duration;

use super::*;

#[test]
fn test_system_clock_now_returns_value() {
    let clock = SystemClock;
    let instant = clock.now();
    let elapsed = clock.elapsed(instant);
    assert!(elapsed < Duration::from_secs(1));
}

#[test]
fn system_time_returns_recent_epoch() {
    let clock = SystemClock;
    let st = clock.system_time();
    assert!(
        st.duration_since(std::time::SystemTime::UNIX_EPOCH).is_ok(),
        "system time before UNIX epoch"
    );
}

#[test]
fn test_generic_function_with_system_clock() {
    fn is_expired<C: GetNow + GetElapsed>(clock: &C, started_at: C::Instant, ttl: Duration) -> bool {
        clock.elapsed(started_at) >= ttl
    }

    let clock = SystemClock;
    let start = clock.now();
    assert!(!is_expired(&clock, start, Duration::from_secs(9999)));
}
