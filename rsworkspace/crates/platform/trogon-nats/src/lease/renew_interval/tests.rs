use super::*;

#[test]
fn from_secs_accepts_non_zero() {
    let interval = LeaseRenewInterval::from_secs(3).unwrap();
    assert_eq!(interval.as_duration(), Duration::from_secs(3));
}

#[test]
fn from_secs_rejects_zero() {
    let err = LeaseRenewInterval::from_secs(0).unwrap_err();
    assert_eq!(err, LeaseRenewIntervalError::ZeroDuration);
}

#[test]
fn from_millis_accepts_non_zero() {
    let interval = LeaseRenewInterval::from_millis(250).unwrap();
    assert_eq!(interval.as_duration(), Duration::from_millis(250));
}

#[test]
fn from_millis_rejects_zero() {
    let err = LeaseRenewInterval::from_millis(0).unwrap_err();
    assert_eq!(err, LeaseRenewIntervalError::ZeroDuration);
}

#[test]
fn new_and_conversions_preserve_value() {
    let non_zero = NonZeroDuration::from_secs(4).unwrap();
    let interval = LeaseRenewInterval::new(non_zero);
    let back_to_non_zero: NonZeroDuration = interval.into();
    let as_duration: Duration = interval.into();

    assert_eq!(Duration::from(back_to_non_zero), Duration::from_secs(4));
    assert_eq!(as_duration, Duration::from_secs(4));
}

#[test]
fn from_non_zero_duration_constructor_preserves_value() {
    let interval = LeaseRenewInterval::from(NonZeroDuration::from_secs(6).unwrap());
    assert_eq!(interval.as_duration(), Duration::from_secs(6));
}

#[test]
fn display_message_is_actionable() {
    assert_eq!(
        LeaseRenewIntervalError::ZeroDuration.to_string(),
        "lease renew interval must not be zero"
    );
}
