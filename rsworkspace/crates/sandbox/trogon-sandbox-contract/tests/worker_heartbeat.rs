use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::{
    ArtifactType, CapabilitySupport, Digest, DormancyCapability, EgressCapability, EgressEnforcementLevel, EnvDigest,
    IngressCapability, IsolationCapability, IsolationTier, LifecycleCapability, NativeVolumeBinding,
    PerformanceProfile, PlacementCapability, ProviderCapabilityProfile, ProviderKind, ResourceCapability,
    ResourceLimitMechanism, ResourceRange, SnapshotCapability, VolumeCapability, WorkerCapabilities, WorkerCapacity,
    WorkerDispatchProtocol, WorkerHeartbeat, WorkerId,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ENV_DIGEST: &str = DIGEST;

fn emitted_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

fn isolation() -> IsolationCapability {
    IsolationCapability {
        default_tier: IsolationTier::Container,
        supported_tiers: vec![IsolationTier::Container, IsolationTier::MicroVm],
        egress_enforcement: EgressEnforcementLevel::Mandatory,
        supports_privileged: CapabilitySupport::Unsupported,
        supports_sandbox_users: CapabilitySupport::Unsupported,
        supports_interactive_stdin: CapabilitySupport::Unsupported,
        supports_named_processes: CapabilitySupport::Unsupported,
        supports_process_readiness: CapabilitySupport::Unsupported,
        supports_graceful_stop: CapabilitySupport::Unsupported,
        supports_command_restart: CapabilitySupport::Unsupported,
        supports_command_cancellation: CapabilitySupport::Unsupported,
        supports_command_launch_cancellation: CapabilitySupport::Unsupported,
        supports_port_preview: CapabilitySupport::Unsupported,
        supports_expose_after_create: CapabilitySupport::Unsupported,
        supports_evidence_streaming: CapabilitySupport::Unsupported,
        supports_output_streaming: CapabilitySupport::Unsupported,
        supports_domain_allowlist: CapabilitySupport::Supported,
        supports_tls_intercept: CapabilitySupport::Supported,
        supports_secret_substitution: CapabilitySupport::Supported,
        supports_secret_bindings: CapabilitySupport::Supported,
        supports_masked_files: CapabilitySupport::Supported,
        supports_fs_policy: CapabilitySupport::Supported,
        supports_file_ops: CapabilitySupport::Unsupported,
        supports_file_search: CapabilitySupport::Unsupported,
        supports_file_archive: CapabilitySupport::Unsupported,
        supports_output_redaction: CapabilitySupport::Supported,
        supports_terminal: CapabilitySupport::Unsupported,
        resource_limits: ResourceLimitMechanism::CgroupV2,
        egress: EgressCapability::PROXIED,
        supports_hardening_overrides: CapabilitySupport::Supported,
        supports_change_tracking: CapabilitySupport::Unsupported,
        supports_file_watch: CapabilitySupport::Unsupported,
        supports_persistent_runtime: CapabilitySupport::Unsupported,
        supports_dir_transfer: CapabilitySupport::Unsupported,
        supports_browser: CapabilitySupport::Unsupported,
        supports_network_policy_update: CapabilitySupport::Unsupported,
        supports_sandbox_groups: CapabilitySupport::Unsupported,
    }
}

fn profile() -> Result<ProviderCapabilityProfile, Box<dyn std::error::Error>> {
    Ok(ProviderCapabilityProfile {
        provider: ProviderKind::Mock,
        snapshot: SnapshotCapability {
            can_snapshot: CapabilitySupport::Unsupported,
            can_checkpoint_running: CapabilitySupport::Unsupported,
            can_restore: CapabilitySupport::Unsupported,
            supported_artifact_types: vec![ArtifactType::SandboxRootfsV1],
            layered_deltas: CapabilitySupport::Unsupported,
            max_snapshot_size_bytes: None,
            supports_native_snapshots: CapabilitySupport::Supported,
            supports_in_place_rollback: CapabilitySupport::Unsupported,
        },
        volume: VolumeCapability {
            supported_modes: Vec::new(),
            supports_s3_backend: CapabilitySupport::Unsupported,
            supports_fuse_mounts: CapabilitySupport::Unsupported,
        },
        isolation: isolation(),
        lifecycle: LifecycleCapability {
            supports_warm_pool: CapabilitySupport::Supported,
            max_warm_pool_size: Some(8),
            supports_named_tags: CapabilitySupport::Supported,
            supports_prewarm_per_tenant: CapabilitySupport::Unsupported,
            supports_pause_resume: CapabilitySupport::Unsupported,
            supports_native_auto_stop: CapabilitySupport::Unsupported,
            max_lease_lifetime_seconds: Some(3_600),
            dormancy: DormancyCapability::FREEZE,
        },
        performance: PerformanceProfile {
            cold_start_p50_ms: 10,
            cold_restore_p50_ms: 20,
            max_concurrent_sandboxes: Some(32),
        },
        resources: ResourceCapability {
            vcpu_millis: Some(ResourceRange::new(100, 64_000)?),
            disk_mib: None,
            reports_usage_gauges: CapabilitySupport::Unsupported,
        },
        placement: PlacementCapability {
            regions: vec!["region-a".to_owned()],
            supports_native_passthrough: CapabilitySupport::Unsupported,
            env_digest: Some(EnvDigest::new(ENV_DIGEST)?),
        },
        ingress: IngressCapability {
            max_exposed_ports: Some(4),
        },
    })
}

fn binding() -> Result<NativeVolumeBinding, serde_json::Error> {
    serde_json::from_value(json!({
        "mount": {
            "drive_id": { "tenant": "acme", "name": "d0" },
            "mount_path": "/mnt/d0",
            "mode": "read_write",
        },
        "storage": { "provider": "fly", "volume_id": "vol_123" },
    }))
}

fn heartbeat() -> Result<WorkerHeartbeat, Box<dyn std::error::Error>> {
    Ok(WorkerHeartbeat {
        worker_id: WorkerId::new("worker-1")?,
        dispatch_protocol: WorkerDispatchProtocol::TargetedV1,
        native_volume_bindings: vec![binding()?],
        provider: ProviderKind::Mock,
        capacity: WorkerCapacity {
            max_concurrent_leases: 8,
            current_leases: 3,
            capabilities: WorkerCapabilities::SUPPORTS_SNAPSHOT,
        },
        emitted_at: emitted_at(),
        cached_digests: vec![Digest::new(DIGEST)?],
        profile: profile()?,
        warm_pool_population: HashMap::from([(Digest::new(DIGEST)?, 2)]),
        supports_lease_epoch: CapabilitySupport::Supported,
    })
}

#[test]
fn worker_heartbeat_round_trips_through_json() -> TestResult {
    let original = heartbeat()?;
    let wire = serde_json::to_value(&original)?;
    assert_eq!(wire["worker_id"], json!("worker-1"));
    assert_eq!(wire["dispatch_protocol"], json!("targeted_v1"));
    assert_eq!(
        wire["native_volume_bindings"][0]["storage"]["volume_id"],
        json!("vol_123")
    );
    assert_eq!(wire["provider"], json!("mock"));
    assert_eq!(
        wire["capacity"],
        json!({ "max_concurrent_leases": 8, "current_leases": 3, "capabilities": 1 })
    );
    assert_eq!(
        wire["emitted_at"],
        json!({ "secs_since_epoch": 1_700_000_000, "nanos_since_epoch": 0 })
    );
    assert_eq!(wire["cached_digests"], json!([DIGEST]));
    assert_eq!(wire["profile"], serde_json::to_value(profile()?)?);
    assert_eq!(wire["warm_pool_population"], json!({ DIGEST: 2 }));
    assert_eq!(wire["supports_lease_epoch"], json!(true));
    assert_eq!(serde_json::from_value::<WorkerHeartbeat>(wire)?, original);
    Ok(())
}

#[test]
fn worker_heartbeat_defaults_fields_older_workers_omit() -> TestResult {
    let mut wire = serde_json::to_value(heartbeat()?)?;
    let object = wire.as_object_mut().ok_or("heartbeat is not an object")?;
    for field in [
        "dispatch_protocol",
        "native_volume_bindings",
        "cached_digests",
        "warm_pool_population",
        "supports_lease_epoch",
    ] {
        assert!(object.remove(field).is_some(), "{field}");
    }
    let capacity = object
        .get_mut("capacity")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or("capacity is not an object")?;
    assert!(capacity.remove("capabilities").is_some());

    let parsed: WorkerHeartbeat = serde_json::from_value(wire)?;
    assert_eq!(parsed.dispatch_protocol, WorkerDispatchProtocol::Legacy);
    assert!(parsed.native_volume_bindings.is_empty());
    assert!(parsed.cached_digests.is_empty());
    assert!(parsed.warm_pool_population.is_empty());
    assert_eq!(parsed.supports_lease_epoch, CapabilitySupport::Unsupported);
    assert_eq!(parsed.capacity.capabilities, WorkerCapabilities::empty());

    let reserialized = serde_json::to_value(&parsed)?;
    assert!(reserialized.get("native_volume_bindings").is_none());
    assert!(reserialized.get("warm_pool_population").is_none());
    assert_eq!(reserialized["supports_lease_epoch"], json!(false));
    Ok(())
}

#[test]
fn worker_heartbeat_rejects_invalid_fields() -> TestResult {
    let valid = serde_json::to_value(heartbeat()?)?;
    for (field, value) in [
        ("worker_id", json!("worker 1")),
        ("worker_id", json!("")),
        ("dispatch_protocol", json!("targeted_v2")),
        ("cached_digests", json!(["sha256:short"])),
        ("warm_pool_population", json!({ "md5:abc": 1 })),
        (
            "capacity",
            json!({ "max_concurrent_leases": 1, "current_leases": 0, "capabilities": 1 << 31 }),
        ),
        ("profile", json!(null)),
    ] {
        let mut wire = valid.clone();
        wire[field] = value;
        let error = serde_json::from_value::<WorkerHeartbeat>(wire)
            .err()
            .ok_or("invalid heartbeat deserialized")?;
        assert_eq!(error.classify(), Data, "{field}");
    }

    let mut missing_profile = valid;
    let object = missing_profile.as_object_mut().ok_or("heartbeat is not an object")?;
    assert!(object.remove("profile").is_some());
    assert!(serde_json::from_value::<WorkerHeartbeat>(missing_profile).is_err());
    Ok(())
}

#[test]
fn worker_dispatch_protocols_use_snake_case_names() -> TestResult {
    assert_eq!(WorkerDispatchProtocol::default(), WorkerDispatchProtocol::Legacy);
    for (protocol, name) in [
        (WorkerDispatchProtocol::Legacy, "legacy"),
        (WorkerDispatchProtocol::TargetedV1, "targeted_v1"),
    ] {
        assert_eq!(serde_json::to_value(protocol)?, json!(name));
        assert_eq!(serde_json::from_value::<WorkerDispatchProtocol>(json!(name))?, protocol);
    }
    Ok(())
}

#[test]
fn worker_capacity_defaults_to_no_leases_and_no_capabilities() -> TestResult {
    assert_eq!(
        serde_json::to_value(WorkerCapacity::default())?,
        json!({ "max_concurrent_leases": 0, "current_leases": 0, "capabilities": 0 })
    );
    Ok(())
}
