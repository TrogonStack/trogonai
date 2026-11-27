use crate::{SnapshotRecord, SnapshotState};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn a_snapshot_reads_with_its_size_truncated() -> TestResult {
    let record: SnapshotRecord = serde_json::from_str(
        r#"{"id":"s1","name":"n","state":"active","size":2.9,"sourceSandboxId":"sbx","organizationId":"o"}"#,
    )?;
    assert_eq!(record.size, Some(2));
    assert_eq!(record.source_sandbox_id.as_deref(), Some("sbx"));
    Ok(())
}

#[test]
fn a_snapshot_without_an_id_or_name_is_refused() {
    assert!(serde_json::from_str::<SnapshotRecord>(r#"{"name":"n"}"#).is_err());
    assert!(serde_json::from_str::<SnapshotRecord>(r#"{"id":"s"}"#).is_err());
    assert!(serde_json::from_str::<SnapshotRecord>(r#"{"id":"","name":"n"}"#).is_err());
}

#[test]
fn only_error_states_are_failures() -> TestResult {
    for (state, failed) in [
        ("error", true),
        ("build_failed", true),
        ("active", false),
        ("brand_new", false),
    ] {
        let parsed: SnapshotState = serde_json::from_str(&format!("\"{state}\""))?;
        assert_eq!(parsed.is_failed(), failed, "{state}");
    }
    Ok(())
}
