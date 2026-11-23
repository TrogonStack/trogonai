use std::collections::HashSet;

use serde_json::error::Category::{Data, Syntax};
use trogon_sandbox_contract::{
    ArtifactType, EgressCapability, EgressEnforcementLevel, ResourceLimitMechanism, SnapshotCapability,
    VolumeCapability, VolumeMode,
};

#[test]
fn enums_preserve_their_wire_spellings_and_reject_unknown_values() -> Result<(), Box<dyn std::error::Error>> {
    let enforcement = [
        (EgressEnforcementLevel::None, "none"),
        (EgressEnforcementLevel::BestEffort, "best_effort"),
        (EgressEnforcementLevel::Mandatory, "mandatory"),
    ];
    let limits = [
        (ResourceLimitMechanism::None, "none"),
        (ResourceLimitMechanism::Rlimit, "rlimit"),
        (ResourceLimitMechanism::CgroupV2, "cgroup_v2"),
        (ResourceLimitMechanism::JobObject, "job_object"),
        (ResourceLimitMechanism::Runtime, "runtime"),
    ];
    let modes = [
        (VolumeMode::ReadOnly, "read_only"),
        (VolumeMode::ReadWrite, "read_write"),
        (VolumeMode::CopyOnWrite, "copy_on_write"),
    ];
    for (value, spelling) in enforcement {
        let json = serde_json::to_string(&value)?;
        assert_eq!(json, format!("\"{spelling}\""));
        assert_eq!(serde_json::from_str::<EgressEnforcementLevel>(&json)?, value);
    }
    for (value, spelling) in limits {
        let json = serde_json::to_string(&value)?;
        assert_eq!(json, format!("\"{spelling}\""));
        assert_eq!(serde_json::from_str::<ResourceLimitMechanism>(&json)?, value);
    }
    for (value, spelling) in modes {
        let json = serde_json::to_string(&value)?;
        assert_eq!(json, format!("\"{spelling}\""));
        assert_eq!(serde_json::from_str::<VolumeMode>(&json)?, value);
    }
    for json in ["\"best-effort\"", "\"cgroup-v2\"", "\"copy_on_write_v2\""] {
        let error = serde_json::from_str::<EgressEnforcementLevel>(json).err();
        assert!(error.is_some(), "decoded {json}");
    }
    for json in ["null", "true", "0", "[]", "{}"] {
        let actual = serde_json::from_str::<VolumeMode>(json)
            .map(|_| None)
            .unwrap_or_else(|error| Some(error.classify()));
        assert_eq!(actual, Some(Syntax), "{json}");
    }
    for json in ["\"unknown\"", "\"read_only_v2\""] {
        let actual = serde_json::from_str::<VolumeMode>(json)
            .map(|_| None)
            .unwrap_or_else(|error| Some(error.classify()));
        assert_eq!(actual, Some(Data), "{json}");
    }
    Ok(())
}

#[test]
fn capability_values_preserve_defaults_and_constants() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(EgressCapability::NONE, EgressCapability::default());
    assert_eq!(EgressCapability::DENY_ONLY.deny_all, true);
    assert_eq!(EgressCapability::PROXIED.host_allowlist, true);
    assert_eq!(ResourceLimitMechanism::default(), ResourceLimitMechanism::None);
    let json = serde_json::to_string(&EgressCapability::PROXIED)?;
    assert_eq!(
        serde_json::from_str::<EgressCapability>(&json)?,
        EgressCapability::PROXIED
    );
    Ok(())
}

#[test]
fn snapshot_and_volume_capabilities_round_trip_with_optional_fields() -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = SnapshotCapability {
        can_snapshot: true,
        can_checkpoint_running: false,
        can_restore: true,
        supported_artifact_types: vec![ArtifactType::SandboxRootfsV1],
        layered_deltas: true,
        max_snapshot_size_bytes: None,
        supports_native_snapshots: true,
        supports_in_place_rollback: false,
    };
    let volume = VolumeCapability {
        supported_modes: vec![VolumeMode::ReadOnly, VolumeMode::CopyOnWrite],
        supports_s3_backend: true,
        supports_fuse_mounts: false,
    };
    assert_eq!(
        serde_json::from_str::<SnapshotCapability>(&serde_json::to_string(&snapshot)?)?,
        snapshot
    );
    assert_eq!(
        serde_json::from_str::<VolumeCapability>(&serde_json::to_string(&volume)?)?,
        volume
    );
    assert!(HashSet::from([VolumeMode::ReadOnly, VolumeMode::CopyOnWrite]).len() == 2);
    Ok(())
}
