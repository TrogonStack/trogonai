use std::error::Error;

use super::*;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn opening_names_the_store_and_keeps_the_cause() -> TestResult {
    let error = ServeError::opening("quota store")(anyhow::anyhow!("bucket missing"));
    assert_eq!(error.to_string(), "open the quota store");
    let source = error.source().ok_or("open error has no source")?;
    assert_eq!(source.to_string(), "bucket missing");
    Ok(())
}

#[test]
fn snapshot_accounting_unavailable_has_no_source() {
    let error = ServeError::SnapshotAccountingUnavailable;
    assert_eq!(error.to_string(), "snapshot accounting unavailable");
    assert!(error.source().is_none());
}
