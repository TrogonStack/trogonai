use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::{
    ArtifactType, EvictedArtifactAction, PrewarmSpec, ProviderKind, RetentionPolicy, RetentionTagLimit,
    RetentionTagLimitError, SnapshotPolicy,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn snapshot_policy_roundtrips_with_every_field() -> TestResult {
    let original = SnapshotPolicy {
        artifact_type: ArtifactType::SandboxRootfsV1,
        target_repository: "snapshots/acme".to_owned(),
        retention: Some(RetentionPolicy {
            max_tags: RetentionTagLimit::new(3)?,
            max_age_days: 30,
            delete_evicted: EvictedArtifactAction::Delete,
        }),
        prewarm: Some(PrewarmSpec {
            pool_size: 3,
            providers: vec![ProviderKind::LocalDocker, ProviderKind::K8sMicroVm],
        }),
        tag_name: Some("nightly".to_owned()),
    };
    let wire = serde_json::to_value(&original)?;
    assert_eq!(
        wire,
        json!({
            "artifact_type": "sandbox_rootfs_v1",
            "target_repository": "snapshots/acme",
            "retention": { "max_tags": 3, "max_age_days": 30, "delete_evicted": true },
            "prewarm": { "pool_size": 3, "providers": ["local_docker", "k8s_micro_vm"] },
            "tag_name": "nightly",
        })
    );
    assert_eq!(serde_json::from_value::<SnapshotPolicy>(wire)?, original);
    Ok(())
}

#[test]
fn snapshot_policy_omits_and_defaults_absent_fields() -> TestResult {
    let wire = json!({ "artifact_type": "sandbox_rootfs_v1", "target_repository": "snapshots/acme" });
    let policy: SnapshotPolicy = serde_json::from_value(wire.clone())?;
    assert_eq!(policy.retention, None);
    assert_eq!(policy.prewarm, None);
    assert_eq!(policy.tag_name, None);
    assert_eq!(serde_json::to_value(&policy)?, wire);
    Ok(())
}

#[test]
fn retention_tag_limit_accepts_values_within_bounds() -> TestResult {
    for value in [RetentionTagLimit::MIN, 5, RetentionTagLimit::MAX] {
        let limit = RetentionTagLimit::new(value)?;
        assert_eq!(limit.get(), value);
        assert_eq!(RetentionTagLimit::try_from(value)?, limit);
        assert_eq!(serde_json::to_value(limit)?, json!(value));
        assert_eq!(serde_json::from_value::<RetentionTagLimit>(json!(value))?, limit);
        assert_eq!(u32::from(limit), value);
    }
    Ok(())
}

#[test]
fn retention_tag_limit_rejects_values_out_of_bounds_at_construction_and_deserialization() {
    let expected = RetentionTagLimitError::OutOfRange { min: 1, max: 10 };
    for value in [0, 11, u32::MAX] {
        assert_eq!(RetentionTagLimit::new(value), Err(expected), "{value}");
        let error = serde_json::from_value::<RetentionTagLimit>(json!(value)).err();
        assert_eq!(
            error.map(|error| error.to_string()),
            Some(expected.to_string()),
            "{value}"
        );
    }
    assert_eq!(expected.to_string(), "max_tags must be between 1 and 10");
}

#[test]
fn retention_policy_rejects_out_of_bounds_max_tags_and_defaults_to_keep() -> TestResult {
    let policy: RetentionPolicy = serde_json::from_value(json!({ "max_tags": 3, "max_age_days": 30 }))?;
    assert_eq!(policy.max_tags.get(), 3);
    assert_eq!(policy.max_age_days, 30);
    assert_eq!(policy.delete_evicted, EvictedArtifactAction::Keep);

    let error = serde_json::from_value::<RetentionPolicy>(json!({ "max_tags": 20, "max_age_days": 30 })).err();
    assert_eq!(
        error.map(|error| error.to_string()),
        Some("max_tags must be between 1 and 10".to_owned())
    );
    Ok(())
}

#[test]
fn evicted_artifact_action_serializes_as_a_json_boolean() -> TestResult {
    for (action, spelling) in [
        (EvictedArtifactAction::Delete, "true"),
        (EvictedArtifactAction::Keep, "false"),
    ] {
        assert_eq!(serde_json::to_string(&action)?, spelling);
        assert_eq!(serde_json::from_str::<EvictedArtifactAction>(spelling)?, action);
        assert_eq!(bool::from(action), action.deletes());
        assert_eq!(EvictedArtifactAction::from(action.deletes()), action);
    }
    assert!(EvictedArtifactAction::Delete.deletes());
    assert!(!EvictedArtifactAction::Keep.deletes());
    assert_eq!(EvictedArtifactAction::default(), EvictedArtifactAction::Keep);
    for json in ["\"true\"", "1", "null"] {
        let actual = serde_json::from_str::<EvictedArtifactAction>(json)
            .map(|_| None)
            .unwrap_or_else(|error| Some(error.classify()));
        assert_eq!(actual, Some(Data), "{json}");
    }
    Ok(())
}
