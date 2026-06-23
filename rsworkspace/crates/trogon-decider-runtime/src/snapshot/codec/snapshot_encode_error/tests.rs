use super::*;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct TestSourceError(&'static str);

#[test]
fn display_and_source_preserve_payload_encode_error() {
    let error = SnapshotEncodeError::<TestSourceError, TestSourceError>::payload(TestSourceError("invalid payload"));

    assert_eq!(error.to_string(), "failed to encode snapshot payload: invalid payload");
    assert_eq!(
        std::error::Error::source(&error).map(ToString::to_string),
        Some("invalid payload".to_string())
    );
    assert!(error.payload_source().is_some());
    assert!(error.snapshot_type_source().is_none());
}

#[test]
fn display_and_source_preserve_snapshot_type_error() {
    let error = SnapshotEncodeError::<TestSourceError, TestSourceError>::snapshot_type(TestSourceError("missing type"));

    assert_eq!(error.to_string(), "failed to resolve snapshot type: missing type");
    assert_eq!(
        std::error::Error::source(&error).map(ToString::to_string),
        Some("missing type".to_string())
    );
    assert!(error.payload_source().is_none());
    assert!(error.snapshot_type_source().is_some());
}
