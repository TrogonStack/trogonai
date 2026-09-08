use super::*;

#[test]
fn wraps_and_reports_a_non_zero_value() {
    let non_zero = NonZeroU32::new(3).unwrap();
    let limit = ConflictRetryLimit::new(non_zero);

    assert_eq!(limit.as_u32(), 3);
    assert_eq!(limit.as_non_zero(), non_zero);
    assert_eq!(ConflictRetryLimit::try_new(3), Ok(limit));
    assert_eq!(ConflictRetryLimit::try_from(3), Ok(limit));
    assert_eq!(u32::from(limit), 3);
    assert_eq!(limit.to_string(), "3");
}

#[test]
fn rejects_zero_with_typed_error() {
    let error = ConflictRetryLimit::try_new(0).unwrap_err();

    assert_eq!(error.value(), 0);
    assert_eq!(
        error.to_string(),
        "conflict retry limit must be greater than zero, got 0"
    );
    let _: &dyn std::error::Error = &error;
}
