use super::*;

#[test]
fn from_secs_valid() {
    let d = NonZeroDuration::from_secs(10).unwrap();
    assert_eq!(Duration::from(d), Duration::from_secs(10));
}

#[test]
fn from_secs_zero_rejected() {
    assert!(matches!(NonZeroDuration::from_secs(0), Err(ZeroDurationError)));
}

#[test]
fn from_secs_const_valid() {
    // Call at runtime (not in a `const` binding) so the body is covered.
    let d = NonZeroDuration::from_secs_const(10);
    assert_eq!(Duration::from(d), Duration::from_secs(10));
}

#[test]
fn from_millis_valid() {
    let d = NonZeroDuration::from_millis(500).unwrap();
    assert_eq!(Duration::from(d), Duration::from_millis(500));
}

#[test]
fn from_millis_zero_rejected() {
    assert!(matches!(NonZeroDuration::from_millis(0), Err(ZeroDurationError)));
}

#[test]
fn error_display() {
    assert_eq!(ZeroDurationError.to_string(), "duration must not be zero");
}

#[test]
fn copy_semantics() {
    let a = NonZeroDuration::from_secs(5).unwrap();
    let b = a;
    assert_eq!(a, b);
}

#[test]
fn ordering() {
    let short = NonZeroDuration::from_secs(1).unwrap();
    let long = NonZeroDuration::from_secs(10).unwrap();
    assert!(short < long);
}

#[test]
fn try_from_duration_rejects_zero() {
    assert_eq!(NonZeroDuration::try_from(Duration::ZERO), Err(ZeroDurationError));
}

#[test]
fn try_from_duration_preserves_positive_durations() {
    for duration in [Duration::from_nanos(1), Duration::from_millis(7), Duration::MAX] {
        assert_eq!(Duration::from(NonZeroDuration::try_from(duration).unwrap()), duration);
    }
}
