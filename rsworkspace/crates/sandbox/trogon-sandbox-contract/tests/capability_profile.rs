use std::collections::HashSet;

use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::{
    ArtifactType, CapabilityProfile, CapabilitySupport, Digest, DigestError, DormancyCapability, EgressCapability,
    EgressEnforcementLevel, EnvDigest, IngressCapability, IsolationCapability, IsolationTier, LifecycleCapability,
    ListCapabilitiesRequest, ListCapabilitiesResponse, PerformanceProfile, PlacementCapability,
    ProviderCapabilityProfile, ProviderKind, ResourceCapability, ResourceLimitMechanism, ResourceRange,
    ResourceRangeError, SdkFeature, SnapshotCapability, SuspendLevel, SuspendRequestLevel, VolumeCapability,
    VolumeMode, WorkerCapabilities, WorkerCapabilitiesError,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ENV_DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

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

fn fully_capable() -> Result<ProviderCapabilityProfile, Box<dyn std::error::Error>> {
    let mut p = profile()?;
    p.lifecycle.supports_pause_resume = CapabilitySupport::Supported;
    p.lifecycle.supports_native_auto_stop = CapabilitySupport::Supported;
    p.isolation.supports_privileged = CapabilitySupport::Supported;
    p.isolation.supports_sandbox_users = CapabilitySupport::Supported;
    p.isolation.supports_interactive_stdin = CapabilitySupport::Supported;
    p.isolation.supports_named_processes = CapabilitySupport::Supported;
    p.isolation.supports_process_readiness = CapabilitySupport::Supported;
    p.isolation.supports_graceful_stop = CapabilitySupport::Supported;
    p.isolation.supports_command_restart = CapabilitySupport::Supported;
    p.isolation.supports_command_cancellation = CapabilitySupport::Supported;
    p.isolation.supports_command_launch_cancellation = CapabilitySupport::Supported;
    p.isolation.supports_port_preview = CapabilitySupport::Supported;
    p.isolation.supports_expose_after_create = CapabilitySupport::Supported;
    p.isolation.supports_evidence_streaming = CapabilitySupport::Supported;
    p.isolation.supports_output_streaming = CapabilitySupport::Supported;
    p.isolation.supports_file_ops = CapabilitySupport::Supported;
    p.isolation.supports_file_search = CapabilitySupport::Supported;
    p.isolation.supports_file_archive = CapabilitySupport::Supported;
    p.isolation.supports_terminal = CapabilitySupport::Supported;
    p.isolation.supports_change_tracking = CapabilitySupport::Supported;
    p.isolation.supports_file_watch = CapabilitySupport::Supported;
    p.isolation.supports_persistent_runtime = CapabilitySupport::Supported;
    p.isolation.supports_dir_transfer = CapabilitySupport::Supported;
    p.isolation.supports_browser = CapabilitySupport::Supported;
    p.isolation.supports_network_policy_update = CapabilitySupport::Supported;
    p.isolation.supports_sandbox_groups = CapabilitySupport::Supported;
    p.volume.supported_modes = vec![VolumeMode::ReadWrite];
    p.volume.supports_s3_backend = CapabilitySupport::Supported;
    p.volume.supports_fuse_mounts = CapabilitySupport::Supported;
    p.snapshot.can_snapshot = CapabilitySupport::Supported;
    p.snapshot.can_checkpoint_running = CapabilitySupport::Supported;
    p.snapshot.can_restore = CapabilitySupport::Supported;
    p.resources.reports_usage_gauges = CapabilitySupport::Supported;
    p.placement.supports_native_passthrough = CapabilitySupport::Supported;
    Ok(p)
}

#[test]
fn provider_profile_round_trips_and_defaults_axes_missing_from_older_peers() -> TestResult {
    let original = profile()?;
    let value = serde_json::to_value(&original)?;
    assert_eq!(value["placement"]["env_digest"], ENV_DIGEST);
    assert_eq!(value["placement"]["regions"], json!(["region-a"]));
    assert_eq!(value["ingress"], json!({ "max_exposed_ports": 4 }));
    assert_eq!(
        value["resources"],
        json!({ "vcpu_millis": { "min": 100, "max": 64_000 }, "reports_usage_gauges": false })
    );
    assert_eq!(
        value["lifecycle"]["dormancy"],
        json!({ "freeze": true, "suspend": null })
    );
    assert_eq!(
        serde_json::from_value::<ProviderCapabilityProfile>(value.clone())?,
        original
    );

    let mut older = value;
    let object = older.as_object_mut().ok_or("profile is not an object")?;
    for axis in ["resources", "placement", "ingress"] {
        assert!(object.remove(axis).is_some(), "{axis}");
    }
    let parsed: ProviderCapabilityProfile = serde_json::from_value(older)?;
    assert_eq!(parsed.resources, ResourceCapability::default());
    assert_eq!(parsed.placement, PlacementCapability::default());
    assert_eq!(parsed.ingress, IngressCapability::default());
    assert!(parsed.ingress.allows(u16::MAX));

    let empty_placement = serde_json::to_value(PlacementCapability::default())?;
    assert_eq!(empty_placement, json!({ "supports_native_passthrough": false }));
    Ok(())
}

#[test]
fn isolation_flags_default_to_unsupported_when_absent() -> TestResult {
    let parsed: IsolationCapability = serde_json::from_value(json!({
        "default_tier": "micro_vm",
        "supported_tiers": ["micro_vm"],
        "egress_enforcement": "best_effort",
    }))?;
    assert_eq!(parsed.default_tier, IsolationTier::MicroVm);
    assert_eq!(parsed.egress, EgressCapability::NONE);
    assert_eq!(parsed.resource_limits, ResourceLimitMechanism::None);
    assert_eq!(parsed.supports_privileged, CapabilitySupport::Unsupported);
    assert_eq!(parsed.supports_terminal, CapabilitySupport::Unsupported);
    assert_eq!(parsed.supports_sandbox_groups, CapabilitySupport::Unsupported);
    for missing in ["default_tier", "supported_tiers", "egress_enforcement"] {
        let mut value = serde_json::to_value(isolation())?;
        let object = value.as_object_mut().ok_or("isolation is not an object")?;
        assert!(object.remove(missing).is_some(), "{missing}");
        let actual = serde_json::from_value::<IsolationCapability>(value)
            .map(|_| None)
            .unwrap_or_else(|error| Some(error.classify()));
        assert_eq!(actual, Some(Data), "{missing}");
    }
    Ok(())
}

#[test]
fn sdk_features_list_each_backed_feature_once_in_canonical_order() -> TestResult {
    let features = fully_capable()?.sdk_features();
    assert_eq!(features, SdkFeature::ALL.to_vec());
    assert_eq!(features.iter().collect::<HashSet<_>>().len(), SdkFeature::ALL.len());
    assert_eq!(profile()?.sdk_features(), Vec::<SdkFeature>::new());

    let mut launch_without_cancel = profile()?;
    launch_without_cancel.isolation.supports_command_launch_cancellation = CapabilitySupport::Supported;
    assert_eq!(launch_without_cancel.sdk_features(), Vec::<SdkFeature>::new());

    let mut restore_only = profile()?;
    restore_only.snapshot.can_restore = CapabilitySupport::Supported;
    assert_eq!(restore_only.sdk_features(), Vec::<SdkFeature>::new());
    restore_only.snapshot.can_checkpoint_running = CapabilitySupport::Supported;
    assert_eq!(restore_only.sdk_features(), vec![SdkFeature::Fork]);

    let mut preview = profile()?;
    preview.isolation.supports_port_preview = CapabilitySupport::Supported;
    assert_eq!(
        preview.sdk_features(),
        vec![SdkFeature::SandboxDomain, SdkFeature::PortPreview]
    );
    Ok(())
}

#[test]
fn sdk_features_preserve_their_wire_spellings() -> TestResult {
    let expected = [
        "pause_resume",
        "write_stdin",
        "restart_command",
        "command_cancellation",
        "command_launch_cancellation",
        "users",
        "drives",
        "snapshots",
        "fork",
        "update_network_policy",
        "sandbox_domain",
        "port_preview",
        "expose_after_create",
        "evidence_streaming",
        "groups",
        "change_tracking",
        "file_watch",
        "dir_transfer",
        "file_ops",
        "file_search",
        "file_archive",
        "terminal",
        "named_processes",
        "graceful_stop",
        "persistent_runtime",
        "browser",
        "output_streaming",
        "usage_gauges",
        "native_auto_stop",
        "native_passthrough",
    ];
    assert_eq!(SdkFeature::ALL.len(), expected.len());
    for (feature, spelling) in SdkFeature::ALL.into_iter().zip(expected) {
        assert_eq!(feature.as_str(), spelling);
        let json = serde_json::to_string(&feature)?;
        assert_eq!(json, format!("\"{spelling}\""));
        assert_eq!(serde_json::from_str::<SdkFeature>(&json)?, feature);
    }
    assert!(serde_json::from_str::<SdkFeature>("\"read_logs\"").is_err());
    Ok(())
}

#[test]
fn tenant_profile_flattens_the_provider_profile() -> TestResult {
    let source = fully_capable()?;
    let summary = CapabilityProfile::from(&source);
    assert_eq!(summary.provider, ProviderKind::Mock);
    assert_eq!(summary.default_isolation_tier, IsolationTier::Container);
    assert_eq!(summary.supported_isolation_tiers, source.isolation.supported_tiers);
    assert_eq!(summary.egress_enforcement, EgressEnforcementLevel::Mandatory);
    assert!(
        summary.can_snapshot.is_supported()
            && summary.can_checkpoint_running.is_supported()
            && summary.can_restore.is_supported()
    );
    assert_eq!(summary.supported_artifact_types, vec![ArtifactType::SandboxRootfsV1]);
    assert!(
        summary.supports_s3_volumes.is_supported()
            && summary.supports_fuse_mounts.is_supported()
            && summary.supports_native_snapshots.is_supported()
    );
    assert_eq!(
        summary.supports_command_launch_cancellation,
        CapabilitySupport::Supported
    );
    assert_eq!(summary.resource_limits, ResourceLimitMechanism::CgroupV2);
    assert_eq!(summary.supported_volume_modes, vec![VolumeMode::ReadWrite]);
    assert_eq!(summary.max_warm_pool_size, Some(8));
    assert_eq!(summary.dormancy, DormancyCapability::FREEZE);
    assert_eq!(
        (
            summary.cold_start_p50_ms,
            summary.cold_restore_p50_ms,
            summary.max_concurrent_sandboxes
        ),
        (10, 20, Some(32))
    );
    assert_eq!(summary.resources, source.resources);
    assert_eq!(summary.egress, EgressCapability::PROXIED);
    assert_eq!(summary.placement, source.placement);
    assert_eq!(summary.ingress, source.ingress);
    assert_eq!(summary.max_lease_lifetime_seconds, Some(3_600));
    assert_eq!(CapabilityProfile::from(source.clone()), summary);
    assert_eq!(
        serde_json::from_value::<CapabilityProfile>(serde_json::to_value(&summary)?)?,
        summary
    );

    let mut launch_without_cancel = profile()?;
    launch_without_cancel.isolation.supports_command_launch_cancellation = CapabilitySupport::Supported;
    assert_eq!(
        CapabilityProfile::from(&launch_without_cancel).supports_command_launch_cancellation,
        CapabilitySupport::Unsupported
    );
    Ok(())
}

#[test]
fn list_capabilities_messages_omit_an_absent_provider_filter() -> TestResult {
    assert_eq!(serde_json::to_value(ListCapabilitiesRequest::default())?, json!({}));
    let filtered = ListCapabilitiesRequest {
        provider: Some(ProviderKind::Firecracker),
    };
    assert_eq!(serde_json::to_value(&filtered)?, json!({ "provider": "firecracker" }));
    assert_eq!(
        serde_json::from_value::<ListCapabilitiesRequest>(json!({}))?,
        ListCapabilitiesRequest::default()
    );

    let response = ListCapabilitiesResponse {
        profiles: vec![CapabilityProfile::from(profile()?)],
    };
    let json = serde_json::to_string(&response)?;
    assert_eq!(serde_json::from_str::<ListCapabilitiesResponse>(&json)?, response);
    assert_eq!(ListCapabilitiesResponse::default().profiles.len(), 0);
    Ok(())
}

#[test]
fn worker_capabilities_derive_from_the_profile_and_serialize_as_bits() -> TestResult {
    assert_eq!(WorkerCapabilities::from(&profile()?), WorkerCapabilities::empty());
    assert!(WorkerCapabilities::default().is_empty());

    let all = WorkerCapabilities::from(&fully_capable()?);
    assert_eq!(all, WorkerCapabilities::all());
    assert_eq!(all.bits(), 511);
    assert_eq!(serde_json::to_string(&all)?, "511");

    let flags = [
        (WorkerCapabilities::SUPPORTS_SNAPSHOT, 1),
        (WorkerCapabilities::SUPPORTS_S3_VOLUMES, 2),
        (WorkerCapabilities::SUPPORTS_RESTORE, 4),
        (WorkerCapabilities::SUPPORTS_FUSE_MOUNTS, 8),
        (WorkerCapabilities::SUPPORTS_PRIVILEGED_EXEC, 16),
        (WorkerCapabilities::SUPPORTS_SANDBOX_USERS, 32),
        (WorkerCapabilities::SUPPORTS_INTERACTIVE_STDIN, 64),
        (WorkerCapabilities::SUPPORTS_NAMED_PROCESSES, 128),
        (WorkerCapabilities::SUPPORTS_PROCESS_READINESS, 256),
    ];
    for (flag, bits) in flags {
        assert_eq!(flag.bits(), bits);
        assert!(all.contains(flag));
        assert!(!WorkerCapabilities::empty().contains(flag));
    }

    let mut partial = profile()?;
    partial.snapshot.can_restore = CapabilitySupport::Supported;
    partial.isolation.supports_named_processes = CapabilitySupport::Supported;
    let mut expected = WorkerCapabilities::SUPPORTS_RESTORE;
    expected |= WorkerCapabilities::SUPPORTS_NAMED_PROCESSES;
    assert_eq!(WorkerCapabilities::from(&partial), expected);
    assert_eq!(u32::from(expected), 132);
    assert_eq!(serde_json::from_str::<WorkerCapabilities>("132")?, expected);
    assert!(!expected.contains(WorkerCapabilities::SUPPORTS_SNAPSHOT));

    assert_eq!(
        WorkerCapabilities::try_from(512),
        Err(WorkerCapabilitiesError::UnknownBits(512))
    );
    assert_eq!(
        WorkerCapabilitiesError::UnknownBits(512).to_string(),
        "unknown worker capability bits: 512"
    );
    for json in ["512", "-1", "\"1\""] {
        assert!(
            serde_json::from_str::<WorkerCapabilities>(json).is_err(),
            "decoded {json}"
        );
    }
    Ok(())
}

#[test]
fn dormancy_resolves_only_the_offered_suspend_level() -> TestResult {
    let requests = [
        SuspendRequestLevel::Auto,
        SuspendRequestLevel::Memory,
        SuspendRequestLevel::Disk,
    ];
    let cases = [
        (DormancyCapability::NONE, [None, None, None]),
        (DormancyCapability::FREEZE, [None, None, None]),
        (
            DormancyCapability::SUSPEND_MEMORY,
            [Some(SuspendLevel::Memory), Some(SuspendLevel::Memory), None],
        ),
        (
            DormancyCapability::SUSPEND_DISK,
            [Some(SuspendLevel::Disk), None, Some(SuspendLevel::Disk)],
        ),
    ];
    for (capability, expected) in cases {
        for (request, level) in requests.into_iter().zip(expected) {
            assert_eq!(capability.resolve(request), level, "{capability:?} {request:?}");
        }
    }
    assert_eq!(DormancyCapability::default(), DormancyCapability::NONE);
    assert_eq!(SuspendRequestLevel::default(), SuspendRequestLevel::Auto);
    for (request, spelling) in requests.into_iter().zip(["auto", "memory", "disk"]) {
        assert_eq!(request.as_str(), spelling);
        assert_eq!(serde_json::to_string(&request)?, format!("\"{spelling}\""));
    }
    for (level, spelling) in [(SuspendLevel::Memory, "memory"), (SuspendLevel::Disk, "disk")] {
        assert_eq!(level.as_str(), spelling);
        assert_eq!(serde_json::from_str::<SuspendLevel>(&format!("\"{spelling}\""))?, level);
    }
    assert!(serde_json::from_str::<SuspendLevel>("\"auto\"").is_err());
    assert_eq!(
        serde_json::from_value::<DormancyCapability>(json!({ "suspend": "disk" }))?,
        DormancyCapability::SUSPEND_DISK
    );
    Ok(())
}

#[test]
fn resource_and_ingress_bounds_are_inclusive() -> TestResult {
    let range = ResourceRange::new(100, 200)?;
    assert_eq!((range.min(), range.max()), (100, 200));
    assert!(range.contains(100) && range.contains(150) && range.contains(200));
    assert!(!range.contains(99) && !range.contains(201));
    let single = ResourceRange::new(7, 7)?;
    assert!(single.contains(7) && !single.contains(6) && !single.contains(8));
    assert_eq!(
        ResourceRange::new(200, 100),
        Err(ResourceRangeError::Reversed { min: 200, max: 100 })
    );
    assert_eq!(
        ResourceRangeError::Reversed { min: 200, max: 100 }.to_string(),
        "resource range minimum 200 exceeds maximum 100"
    );
    assert_eq!(serde_json::to_value(range)?, json!({"min": 100, "max": 200}));
    assert_eq!(
        serde_json::from_value::<ResourceRange>(json!({"min": 7, "max": 7}))?,
        single
    );
    let reversed = serde_json::from_value::<ResourceRange>(json!({"min": 200, "max": 100}))
        .map(|_| None)
        .unwrap_or_else(|error| Some(error.classify()));
    assert_eq!(reversed, Some(Data));
    let reversed_in_profile =
        serde_json::from_value::<ResourceCapability>(json!({"vcpu_millis": {"min": 2, "max": 1}}));
    assert!(reversed_in_profile.is_err());

    let unstated = IngressCapability::default();
    assert!(unstated.allows(u16::MAX));
    let limited = IngressCapability {
        max_exposed_ports: Some(1),
    };
    assert!(limited.allows(0) && limited.allows(1) && !limited.allows(2));
    let none = IngressCapability {
        max_exposed_ports: Some(0),
    };
    assert!(none.allows(0) && !none.allows(1));
    Ok(())
}

#[test]
fn env_digest_reuses_the_digest_grammar() -> TestResult {
    let parsed: EnvDigest = ENV_DIGEST.parse()?;
    assert_eq!(parsed.as_str(), ENV_DIGEST);
    assert_eq!(parsed.to_string(), ENV_DIGEST);
    assert_eq!(EnvDigest::from(Digest::new(ENV_DIGEST)?), parsed);
    assert_eq!(serde_json::to_string(&parsed)?, format!("\"{ENV_DIGEST}\""));
    assert_eq!(serde_json::from_str::<EnvDigest>(&format!("\"{ENV_DIGEST}\""))?, parsed);
    assert_eq!(EnvDigest::new("0123"), Err(DigestError::MissingPrefix));
    assert_eq!("sha256:XYZ".parse::<EnvDigest>(), Err(DigestError::InvalidHash));
    assert!(serde_json::from_str::<EnvDigest>("\"sha256:abc\"").is_err());
    Ok(())
}

fn changed_tenant_fields(
    before: &CapabilityProfile,
    after: &CapabilityProfile,
) -> Result<Vec<String>, serde_json::Error> {
    let before = serde_json::to_value(before)?;
    let after = serde_json::to_value(after)?;
    let mut changed: Vec<String> = after
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, value)| before.get(key.as_str()) != Some(*value))
        .map(|(key, _)| key.clone())
        .collect();
    changed.sort();
    Ok(changed)
}

#[test]
fn each_profile_flag_backs_exactly_its_own_feature_bit_and_tenant_field() -> TestResult {
    let none = WorkerCapabilities::empty();
    let cases: [(&str, &[SdkFeature], WorkerCapabilities, &[&str]); 33] = [
        (
            "/lifecycle/supports_pause_resume",
            &[SdkFeature::PauseResume],
            none,
            &["supports_pause_resume"],
        ),
        (
            "/lifecycle/supports_native_auto_stop",
            &[SdkFeature::NativeAutoStop],
            none,
            &["supports_native_auto_stop"],
        ),
        (
            "/lifecycle/supports_prewarm_per_tenant",
            &[],
            none,
            &["supports_prewarm_per_tenant"],
        ),
        (
            "/snapshot/can_snapshot",
            &[SdkFeature::Snapshots],
            WorkerCapabilities::SUPPORTS_SNAPSHOT,
            &["can_snapshot"],
        ),
        (
            "/snapshot/can_checkpoint_running",
            &[],
            none,
            &["can_checkpoint_running"],
        ),
        (
            "/snapshot/can_restore",
            &[],
            WorkerCapabilities::SUPPORTS_RESTORE,
            &["can_restore"],
        ),
        (
            "/volume/supports_s3_backend",
            &[],
            WorkerCapabilities::SUPPORTS_S3_VOLUMES,
            &["supports_s3_volumes"],
        ),
        (
            "/volume/supports_fuse_mounts",
            &[],
            WorkerCapabilities::SUPPORTS_FUSE_MOUNTS,
            &["supports_fuse_mounts"],
        ),
        (
            "/isolation/supports_privileged",
            &[],
            WorkerCapabilities::SUPPORTS_PRIVILEGED_EXEC,
            &[],
        ),
        (
            "/isolation/supports_sandbox_users",
            &[SdkFeature::Users],
            WorkerCapabilities::SUPPORTS_SANDBOX_USERS,
            &[],
        ),
        (
            "/isolation/supports_interactive_stdin",
            &[SdkFeature::WriteStdin],
            WorkerCapabilities::SUPPORTS_INTERACTIVE_STDIN,
            &["supports_interactive_stdin"],
        ),
        (
            "/isolation/supports_named_processes",
            &[SdkFeature::NamedProcesses],
            WorkerCapabilities::SUPPORTS_NAMED_PROCESSES,
            &["supports_named_processes"],
        ),
        (
            "/isolation/supports_process_readiness",
            &[],
            WorkerCapabilities::SUPPORTS_PROCESS_READINESS,
            &["supports_process_readiness"],
        ),
        (
            "/isolation/supports_graceful_stop",
            &[SdkFeature::GracefulStop],
            none,
            &["supports_graceful_stop"],
        ),
        (
            "/isolation/supports_command_restart",
            &[SdkFeature::RestartCommand],
            none,
            &["supports_command_restart"],
        ),
        (
            "/isolation/supports_command_cancellation",
            &[SdkFeature::CommandCancellation],
            none,
            &["supports_command_cancellation"],
        ),
        ("/isolation/supports_command_launch_cancellation", &[], none, &[]),
        (
            "/isolation/supports_port_preview",
            &[SdkFeature::SandboxDomain, SdkFeature::PortPreview],
            none,
            &["supports_port_preview"],
        ),
        (
            "/isolation/supports_expose_after_create",
            &[SdkFeature::ExposeAfterCreate],
            none,
            &["supports_expose_after_create"],
        ),
        (
            "/isolation/supports_evidence_streaming",
            &[SdkFeature::EvidenceStreaming],
            none,
            &["supports_evidence_streaming"],
        ),
        (
            "/isolation/supports_output_streaming",
            &[SdkFeature::OutputStreaming],
            none,
            &[],
        ),
        (
            "/isolation/supports_file_ops",
            &[SdkFeature::FileOps],
            none,
            &["supports_file_ops"],
        ),
        (
            "/isolation/supports_file_search",
            &[SdkFeature::FileSearch],
            none,
            &["supports_file_search"],
        ),
        (
            "/isolation/supports_file_archive",
            &[SdkFeature::FileArchive],
            none,
            &["supports_file_archive"],
        ),
        (
            "/isolation/supports_terminal",
            &[SdkFeature::Terminal],
            none,
            &["supports_terminal"],
        ),
        (
            "/isolation/supports_change_tracking",
            &[SdkFeature::ChangeTracking],
            none,
            &["supports_change_tracking"],
        ),
        (
            "/isolation/supports_file_watch",
            &[SdkFeature::FileWatch],
            none,
            &["supports_file_watch"],
        ),
        (
            "/isolation/supports_persistent_runtime",
            &[SdkFeature::PersistentRuntime],
            none,
            &["supports_persistent_runtime"],
        ),
        (
            "/isolation/supports_dir_transfer",
            &[SdkFeature::DirTransfer],
            none,
            &["supports_dir_transfer"],
        ),
        (
            "/isolation/supports_browser",
            &[SdkFeature::Browser],
            none,
            &["supports_browser"],
        ),
        (
            "/isolation/supports_network_policy_update",
            &[SdkFeature::UpdateNetworkPolicy],
            none,
            &["supports_network_policy_update"],
        ),
        (
            "/isolation/supports_sandbox_groups",
            &[SdkFeature::Groups],
            none,
            &["supports_sandbox_groups"],
        ),
        (
            "/resources/reports_usage_gauges",
            &[SdkFeature::UsageGauges],
            none,
            &["resources"],
        ),
    ];
    let baseline = profile()?;
    assert!(baseline.sdk_features().is_empty());
    assert!(WorkerCapabilities::from(&baseline).is_empty());
    let baseline_tenant = CapabilityProfile::from(&baseline);
    let baseline_value = serde_json::to_value(&baseline)?;
    for (pointer, features, bits, tenant_fields) in cases {
        let mut value = baseline_value.clone();
        let flag = value.pointer_mut(pointer).ok_or(pointer)?;
        assert_eq!(*flag, json!(false), "{pointer}");
        *flag = json!(true);
        let flipped: ProviderCapabilityProfile = serde_json::from_value(value)?;
        assert_eq!(flipped.sdk_features(), features, "{pointer}");
        assert_eq!(WorkerCapabilities::from(&flipped), bits, "{pointer}");
        let changed = changed_tenant_fields(&baseline_tenant, &CapabilityProfile::from(&flipped))?;
        assert_eq!(changed, tenant_fields, "{pointer}");
    }

    let mut passthrough = baseline.clone();
    passthrough.placement.supports_native_passthrough = CapabilitySupport::Supported;
    assert_eq!(passthrough.sdk_features(), [SdkFeature::NativePassthrough]);

    let mut drives = baseline.clone();
    drives.volume.supported_modes = vec![VolumeMode::ReadOnly];
    assert_eq!(drives.sdk_features(), [SdkFeature::Drives]);

    let mut fork = baseline.clone();
    fork.snapshot.can_checkpoint_running = CapabilitySupport::Supported;
    fork.snapshot.can_restore = CapabilitySupport::Supported;
    assert_eq!(fork.sdk_features(), [SdkFeature::Fork]);

    let mut launch = baseline;
    launch.isolation.supports_command_cancellation = CapabilitySupport::Supported;
    launch.isolation.supports_command_launch_cancellation = CapabilitySupport::Supported;
    assert_eq!(
        launch.sdk_features(),
        [SdkFeature::CommandCancellation, SdkFeature::CommandLaunchCancellation]
    );
    let tenant = CapabilityProfile::from(&launch);
    assert_eq!(
        tenant.supports_command_launch_cancellation,
        CapabilitySupport::Supported
    );
    Ok(())
}

#[test]
fn tenant_profile_accepts_a_minimal_peer_and_omits_absent_limits() -> TestResult {
    let parsed: CapabilityProfile = serde_json::from_value(json!({
        "provider": "mock",
        "default_isolation_tier": "container",
        "supported_isolation_tiers": ["container"],
        "egress_enforcement": "mandatory",
        "can_snapshot": true,
        "can_checkpoint_running": false,
        "can_restore": true,
        "supported_artifact_types": ["sandbox_rootfs_v1"],
        "supports_s3_volumes": false,
        "supports_fuse_mounts": false,
        "supports_port_preview": true,
        "supports_browser": false,
        "supported_volume_modes": ["read_write"],
        "supports_warm_pool": false,
        "supports_named_tags": false,
        "supports_prewarm_per_tenant": false,
        "cold_start_p50_ms": 5,
        "cold_restore_p50_ms": 7,
    }))?;
    assert_eq!(parsed.provider, ProviderKind::Mock);
    assert_eq!(parsed.supported_volume_modes, [VolumeMode::ReadWrite]);
    assert_eq!(parsed.dormancy, DormancyCapability::NONE);
    assert_eq!(parsed.resource_limits, ResourceLimitMechanism::None);
    assert_eq!(parsed.egress, EgressCapability::NONE);
    assert_eq!(parsed.resources, ResourceCapability::default());
    assert_eq!(parsed.placement, PlacementCapability::default());
    assert_eq!(parsed.ingress, IngressCapability::default());
    assert_eq!(
        parsed.supports_command_launch_cancellation,
        CapabilitySupport::Unsupported
    );
    assert_eq!(parsed.supports_native_auto_stop, CapabilitySupport::Unsupported);

    let value = serde_json::to_value(&parsed)?;
    let object = value.as_object().ok_or("tenant profile is not an object")?;
    for absent in [
        "max_warm_pool_size",
        "max_concurrent_sandboxes",
        "max_lease_lifetime_seconds",
    ] {
        assert!(!object.contains_key(absent), "{absent}");
    }
    assert_eq!(serde_json::from_value::<CapabilityProfile>(value)?, parsed);

    for required in ["provider", "supports_port_preview", "cold_restore_p50_ms"] {
        let mut value = serde_json::to_value(&parsed)?;
        let object = value.as_object_mut().ok_or("tenant profile is not an object")?;
        assert!(object.remove(required).is_some(), "{required}");
        assert!(
            serde_json::from_value::<CapabilityProfile>(value).is_err(),
            "{required}"
        );
    }
    Ok(())
}
