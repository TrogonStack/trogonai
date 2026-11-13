use std::time::Duration;
use trogon_claude_agent_sdk::{Timeout, TimeoutError};

#[test]
fn timeouts_preserve_exact_durations_and_documented_defaults() -> Result<(), TimeoutError> {
    for duration in [Duration::from_nanos(1), Duration::from_secs(7), Timeout::MAX] {
        let value = Timeout::new(duration)?;
        assert_eq!(value.as_duration(), duration);
        assert_eq!(Timeout::try_from(duration)?, value);
    }
    assert_eq!(Timeout::default().as_duration(), Duration::from_secs(60));
    assert_eq!(Timeout::SHUTDOWN_DEFAULT.as_duration(), Duration::from_secs(5));
    Ok(())
}

#[test]
fn unsupported_timeouts_are_typed_errors_before_a_wait_is_started() {
    assert!(matches!(Timeout::new(Duration::ZERO), Err(TimeoutError::Zero)));
    assert!(matches!(
        Timeout::new(Timeout::MAX + Duration::from_nanos(1)),
        Err(TimeoutError::TooLarge)
    ));
    assert!(matches!(Timeout::try_from(Duration::MAX), Err(TimeoutError::TooLarge)));
}
