use super::*;

#[test]
fn wraps_and_reports_a_non_zero_value() {
    let non_zero = NonZeroU64::new(256).unwrap();
    let chunk_size = ReplayChunkSize::new(non_zero);

    assert_eq!(chunk_size.as_u64(), 256);
    assert_eq!(chunk_size.as_non_zero(), non_zero);
    assert_eq!(ReplayChunkSize::try_new(256), Ok(chunk_size));
    assert_eq!(ReplayChunkSize::try_from(256), Ok(chunk_size));
    assert_eq!(u64::from(chunk_size), 256);
    assert_eq!(chunk_size.to_string(), "256");
}

#[test]
fn rejects_zero_with_typed_error() {
    let error = ReplayChunkSize::try_new(0).unwrap_err();

    assert_eq!(error.value(), 0);
    assert_eq!(error.to_string(), "replay chunk size must be greater than zero, got 0");
    let _: &dyn std::error::Error = &error;
}
