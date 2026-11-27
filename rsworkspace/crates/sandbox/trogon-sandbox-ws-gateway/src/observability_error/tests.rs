use std::error::Error;

use super::ObservabilityError;

#[derive(Debug, thiserror::Error)]
#[error("the read failed")]
struct ReadFailedError;

#[test]
fn a_read_failure_keeps_its_source() {
    let error = ObservabilityError::read(ReadFailedError);
    assert!(matches!(error, ObservabilityError::Read(_)));
    assert_eq!(
        error.source().map(ToString::to_string).as_deref(),
        Some("the read failed")
    );
}
