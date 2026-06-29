use super::*;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct TestSourceError(&'static str);

#[test]
fn display_and_source_preserve_payload_decode_error() {
    let error = SnapshotDecodeError::<TestSourceError, TestSourceError>::payload(TestSourceError("invalid payload"));

    assert_eq!(error.to_string(), "failed to decode snapshot payload: invalid payload");
    assert_eq!(
        std::error::Error::source(&error).map(ToString::to_string),
        Some("invalid payload".to_string())
    );
    assert!(error.payload_source().is_some());
    assert!(error.snapshot_type_source().is_none());
}

#[test]
fn display_and_source_preserve_snapshot_type_error() {
    let error = SnapshotDecodeError::<TestSourceError, TestSourceError>::snapshot_type(TestSourceError("missing type"));

    assert_eq!(error.to_string(), "failed to resolve snapshot type: missing type");
    assert_eq!(
        std::error::Error::source(&error).map(ToString::to_string),
        Some("missing type".to_string())
    );
    assert!(error.payload_source().is_none());
    assert!(error.snapshot_type_source().is_some());
}

#[test]
fn display_reports_unexpected_snapshot_type() {
    let error = SnapshotDecodeError::<TestSourceError, TestSourceError>::unexpected_type(
        SnapshotTypeName::new("expected.snapshot").unwrap(),
        "actual.snapshot".to_string(),
    );

    assert_eq!(
        error.to_string(),
        "unexpected snapshot type: expected expected.snapshot, got actual.snapshot"
    );
    assert!(std::error::Error::source(&error).is_none());
    assert!(error.payload_source().is_none());
    assert!(error.snapshot_type_source().is_none());
}
