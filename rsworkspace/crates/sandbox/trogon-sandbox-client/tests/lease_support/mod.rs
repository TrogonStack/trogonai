//! Sample wire values for the lease tests.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::time::{Duration, SystemTime};

use trogon_sandbox_contract::{
    BootMode, LeaseCreation, LeaseGranted, LeaseId, LeasePersistence, LeasePrewarm, LeaseRequest, LeaseRequestKey,
    LeaseStatus, LeaseTtl, LeaseUsage, OciImageRef, OperationRecord, PlatformOptions, PrivilegedAccess, ProviderKind,
    RetentionToggles, SandboxImage, SandboxSpec, SandboxSpecParts, SandboxTags, SecretBindings, WorkspaceSource,
};

use crate::common::tenant;

pub fn sample_request() -> Result<LeaseRequest, Box<dyn std::error::Error>> {
    let spec = SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified("ubuntu:22.04"),
            pull_secret: None,
        },
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: NonZeroU32::new(60).ok_or("cpu")?,
        memory_mib: NonZeroU32::new(512).ok_or("memory")?,
        snapshot_out: None,
        network_policy: None,
        mounts: Vec::new(),
        drives: Vec::new(),
        exposed_ports: Vec::new(),
        boot_mode: BootMode::default(),
        working_dir: None,
        user: None,
        privileged: PrivilegedAccess::default(),
        wasi_capabilities: None,
        fs_policy: None,
        secret_bindings: SecretBindings::default(),
        platform: PlatformOptions::default(),
        idle_timeout_seconds: None,
        no_ttl: LeaseTtl::default(),
        idle_policy: None,
        vcpu_millis: None,
        disk_mib: None,
        retention: RetentionToggles::default(),
    })?;
    Ok(LeaseRequest {
        operation_id: None,
        tenant: tenant()?,
        provider_pref: vec![ProviderKind::Mock],
        spec,
        ttl_seconds: 300,
        idempotency_key: LeaseRequestKey::new("key-1")?,
        isolation_tier: None,
        host_imports: Vec::new(),
        required_capabilities: None,
        prewarm: LeasePrewarm::Regular,
        tags: SandboxTags::default(),
        name: None,
        persistent: LeasePersistence::Persistent,
        native: None,
        region: None,
        fallback_regions: Vec::new(),
    })
}

pub fn sample_granted() -> LeaseGranted {
    LeaseGranted {
        entity_id: None,
        operation_id: None,
        lease_id: LeaseId::new("lease-abc"),
        provider: ProviderKind::Mock,
        sandbox_id: "sandbox-1".into(),
        evidence_subject: "sandbox.evidence.lease-abc.>".into(),
        deadline: SystemTime::UNIX_EPOCH + Duration::from_secs(300),
        created: LeaseCreation::Created,
        capabilities: Vec::new(),
    }
}

pub fn sample_usage() -> Result<LeaseUsage, Box<dyn std::error::Error>> {
    Ok(LeaseUsage {
        lease_id: LeaseId::new("lease-1"),
        tenant: tenant()?,
        status: LeaseStatus::Running,
        created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
        ended_at: None,
        last_activity_at: SystemTime::UNIX_EPOCH + Duration::from_secs(2),
        commands_run: 3,
        command_output_bytes: 40,
        file_bytes_in: 5,
        file_bytes_out: 6,
        running_seconds: 70,
        paused_seconds: 0,
        suspended_seconds: 0,
        suspend_artifact_bytes: None,
        gauges: None,
    })
}

pub fn sample_operation(id: &str) -> Result<OperationRecord, serde_json::Error> {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "tenant": "tenant-1",
        "kind": "pause",
        "request_fingerprint": "abc",
        "state": "succeeded",
        "requested_at": {"secs_since_epoch": 5, "nanos_since_epoch": 0},
    }))
}

pub fn unix(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}
