use std::time::Duration;
use trogon_claude_agent_sdk::{Timeout, TimeoutError};

#[test]
fn timeouts_preserve_exact_durations_and_documented_defaults() -> Result<(), TimeoutError> {
    for duration in [Duration::from_nanos(1), Duration::from_secs(7), Timeout::MAX] {
        let value = Timeout::new(duration);
        assert_eq!(value.as_duration(), duration);
        assert_eq!(Timeout::from(duration), value);
        assert_eq!(Timeout::checked(duration)?, value);
    }
    assert_eq!(Timeout::default().as_duration(), Duration::from_secs(60));
    Ok(())
}

#[test]
fn checked_timeouts_reject_values_outside_the_supported_range() {
    assert!(matches!(Timeout::checked(Duration::ZERO), Err(TimeoutError::Zero)));
    assert!(matches!(
        Timeout::checked(Timeout::MAX + Duration::from_nanos(1)),
        Err(TimeoutError::TooLarge)
    ));
    assert!(matches!(Timeout::checked(Duration::MAX), Err(TimeoutError::TooLarge)));
}
