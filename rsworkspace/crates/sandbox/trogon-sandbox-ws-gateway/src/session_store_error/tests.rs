use std::error::Error;

use super::SessionStoreError;

#[derive(Debug, thiserror::Error)]
#[error("the store failed")]
struct StoreFailedError;

#[test]
fn a_backend_failure_keeps_its_source() {
    let error = SessionStoreError::backend(StoreFailedError);
    assert!(matches!(error, SessionStoreError::Backend(_)));
    assert_eq!(
        error.source().map(ToString::to_string).as_deref(),
        Some("the store failed")
    );
}
