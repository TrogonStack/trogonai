use super::*;

#[test]
fn from_secs_accepts_whole_seconds() {
    let ttl = LeaseTtl::from_secs(2).unwrap();
    assert_eq!(ttl.as_duration(), Duration::from_secs(2));
}

#[test]
fn from_secs_rejects_zero() {
    let err = LeaseTtl::from_secs(0).unwrap_err();
    assert_eq!(err, LeaseTtlError::ZeroDuration);
}

#[test]
fn from_millis_rejects_subsecond_values() {
    let err = LeaseTtl::from_millis(500).unwrap_err();
    assert_eq!(err, LeaseTtlError::SubsecondPrecisionUnsupported);
}

#[test]
fn from_millis_rejects_non_whole_seconds() {
    let err = LeaseTtl::from_millis(1500).unwrap_err();
    assert_eq!(err, LeaseTtlError::SubsecondPrecisionUnsupported);
}

#[test]
fn from_millis_accepts_exact_whole_seconds() {
    let ttl = LeaseTtl::from_millis(2000).unwrap();
    assert_eq!(ttl.as_duration(), Duration::from_secs(2));
}

#[test]
fn from_millis_rejects_zero() {
    let err = LeaseTtl::from_millis(0).unwrap_err();
    assert_eq!(err, LeaseTtlError::ZeroDuration);
}

#[test]
fn try_from_non_zero_duration_rejects_subsecond_values() {
    let value = NonZeroDuration::from_millis(250).unwrap();
    let err = LeaseTtl::try_from(value).unwrap_err();
    assert_eq!(err, LeaseTtlError::SubsecondPrecisionUnsupported);
}

#[test]
fn conversions_preserve_whole_second_value() {
    let ttl = LeaseTtl::from_secs(5).unwrap();
    let non_zero: NonZeroDuration = ttl.into();
    let duration: Duration = ttl.into();

    assert_eq!(Duration::from(non_zero), Duration::from_secs(5));
    assert_eq!(duration, Duration::from_secs(5));
}

#[test]
fn display_messages_are_actionable() {
    assert_eq!(LeaseTtlError::ZeroDuration.to_string(), "lease ttl must not be zero");
    assert_eq!(
        LeaseTtlError::SubsecondPrecisionUnsupported.to_string(),
        "lease ttl must be whole seconds"
    );
}
