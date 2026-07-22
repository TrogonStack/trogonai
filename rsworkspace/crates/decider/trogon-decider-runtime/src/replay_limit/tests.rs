use super::*;

#[test]
fn wraps_and_reports_a_non_zero_value() {
    let non_zero = NonZeroU64::new(42).unwrap();
    let limit = ReplayLimit::new(non_zero);

    assert_eq!(limit.as_u64(), 42);
    assert_eq!(limit.as_non_zero(), non_zero);
    assert_eq!(ReplayLimit::try_new(42), Ok(limit));
    assert_eq!(ReplayLimit::try_from(42), Ok(limit));
    assert_eq!(u64::from(limit), 42);
    assert_eq!(limit.to_string(), "42");
}

#[test]
fn rejects_zero_with_typed_error() {
    let error = ReplayLimit::try_new(0).unwrap_err();

    assert_eq!(error.value(), 0);
    assert_eq!(error.to_string(), "replay limit must be greater than zero, got 0");
    let _: &dyn std::error::Error = &error;
}
