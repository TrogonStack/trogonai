use std::time::Duration;

use super::super::{GetElapsed, GetNow};
use super::*;

#[test]
fn test_mock_clock_starts_at_zero() {
    let clock = MockClock::new();
    assert_eq!(clock.current_time(), Duration::ZERO);
    assert_eq!(clock.now(), MockInstant(Duration::ZERO));
}

#[test]
fn test_mock_clock_advance() {
    let clock = MockClock::new();

    clock.advance(Duration::from_millis(100));
    assert_eq!(clock.current_time(), Duration::from_millis(100));

    clock.advance(Duration::from_millis(200));
    assert_eq!(clock.current_time(), Duration::from_millis(300));
}

#[test]
fn test_mock_clock_set() {
    let clock = MockClock::new();

    clock.set(Duration::from_secs(42));
    assert_eq!(clock.current_time(), Duration::from_secs(42));

    clock.set(Duration::from_secs(10));
    assert_eq!(clock.current_time(), Duration::from_secs(10));
}

#[test]
fn test_mock_clock_elapsed() {
    let clock = MockClock::new();
    let start = clock.now();

    clock.advance(Duration::from_secs(5));
    assert_eq!(clock.elapsed(start), Duration::from_secs(5));

    clock.advance(Duration::from_secs(3));
    assert_eq!(clock.elapsed(start), Duration::from_secs(8));
}

#[test]
fn test_mock_clock_elapsed_at_zero() {
    let clock = MockClock::new();
    let start = clock.now();
    assert_eq!(clock.elapsed(start), Duration::ZERO);
}

#[test]
fn test_mock_clock_elapsed_saturates() {
    let clock = MockClock::new();
    clock.set(Duration::from_secs(10));
    let later = clock.now();

    clock.set(Duration::from_secs(5));
    assert_eq!(clock.elapsed(later), Duration::ZERO);
}

#[test]
fn test_mock_clock_multiple_instants() {
    let clock = MockClock::new();

    let t0 = clock.now();
    clock.advance(Duration::from_secs(1));
    let t1 = clock.now();
    clock.advance(Duration::from_secs(2));
    let t2 = clock.now();

    assert_eq!(clock.elapsed(t0), Duration::from_secs(3));
    assert_eq!(clock.elapsed(t1), Duration::from_secs(2));
    assert_eq!(clock.elapsed(t2), Duration::ZERO);
}

#[test]
fn test_mock_clock_precise_boundaries() {
    let clock = MockClock::new();
    let start = clock.now();
    let ttl = Duration::from_secs(5);

    clock.set(Duration::from_nanos(4_999_999_999));
    assert!(clock.elapsed(start) < ttl);

    clock.set(Duration::from_secs(5));
    assert!(clock.elapsed(start) >= ttl);

    clock.set(Duration::from_nanos(5_000_000_001));
    assert!(clock.elapsed(start) > ttl);
}

#[test]
fn test_mock_clock_clone_shares_state() {
    let clock = MockClock::new();
    let clone = clock.clone();

    clock.advance(Duration::from_secs(5));
    assert_eq!(clone.current_time(), Duration::from_secs(5));

    clone.advance(Duration::from_secs(3));
    assert_eq!(clock.current_time(), Duration::from_secs(8));
}

#[test]
fn test_mock_clock_default() {
    let clock = MockClock::default();
    assert_eq!(clock.current_time(), Duration::ZERO);
}

#[test]
fn test_mock_instant_ordering() {
    let a = MockInstant(Duration::from_secs(1));
    let b = MockInstant(Duration::from_secs(2));

    assert!(a < b);
    assert!(b > a);
    assert_eq!(a, MockInstant(Duration::from_secs(1)));
}

#[test]
fn test_generic_function_with_mock_clock() {
    fn is_expired<C: GetNow + GetElapsed>(clock: &C, started_at: C::Instant, ttl: Duration) -> bool {
        clock.elapsed(started_at) >= ttl
    }

    let clock = MockClock::new();
    let start = clock.now();
    let ttl = Duration::from_secs(30);

    assert!(!is_expired(&clock, start, ttl));

    clock.advance(Duration::from_secs(29));
    assert!(!is_expired(&clock, start, ttl));

    clock.advance(Duration::from_secs(1));
    assert!(is_expired(&clock, start, ttl));
}
