//! Persisted denial is explicit provenance, never a reinterpretation of old failure.

use trogonai_harness_types::result::ToolDeferral;
use trogonai_harness_types::{PersistedToolResult, PersistedToolStatus, ToolCallId, ToolName, ToolStatus};

#[test]
fn new_denials_roundtrip_without_reclassifying_legacy_or_cancelled_failures() {
    for (live, encoded) in [
        (ToolStatus::Denied, "\"denied\""),
        (ToolStatus::Error, "\"failure\""),
        (ToolStatus::Cancelled, "\"failure\""),
        (ToolStatus::Success, "\"success\""),
    ] {
        let status = PersistedToolStatus::from(live);
        assert_eq!(serde_json::to_string(&status).unwrap(), encoded);
        assert_eq!(serde_json::from_str::<PersistedToolStatus>(encoded).unwrap(), status);
        assert_eq!(status.succeeded(), live == ToolStatus::Success);
    }
    assert_eq!(
        serde_json::from_str::<PersistedToolStatus>("\"failure\"").unwrap(),
        PersistedToolStatus::Failure
    );
    for unknown in ["\"unknown\"", "\"Denied\"", "null", "true", "2"] {
        assert!(serde_json::from_str::<PersistedToolStatus>(unknown).is_err());
    }
}

#[test]
fn explicit_denial_does_not_become_a_deferral_when_its_body_matches_one() {
    let denied = PersistedToolResult::new(
        ToolCallId::new("denied").unwrap(),
        ToolName::new("read_file").unwrap(),
        ToolStatus::Denied.into(),
        ToolDeferral::NotExecuted.output(),
    );
    assert_eq!(denied.deferral(), None);
    let legacy = PersistedToolResult::new(
        ToolCallId::new("legacy").unwrap(),
        ToolName::new("read_file").unwrap(),
        PersistedToolStatus::Failure,
        ToolDeferral::NotExecuted.output(),
    );
    assert_eq!(legacy.deferral(), Some(ToolDeferral::NotExecuted));
}
