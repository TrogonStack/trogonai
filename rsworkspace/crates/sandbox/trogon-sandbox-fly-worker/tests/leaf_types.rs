use std::num::NonZeroU32;
use std::time::Duration;

use serde_json::json;
use trogon_sandbox_contract::{DriveId, MountPath, ProviderVolumeIdError};
use trogon_sandbox_fly_worker::{
    ExecOutput, FlyApiToken, FlyApiTokenError, FlyAppName, FlyAppNameError, FlyConfig, FlyConfigError, FlyConfigParts,
    FlyEndpoint, FlyEndpointError, FlyPreviewNetwork, FlyRegion, FlyRegionError, FlySnapshot, FlySnapshotRef,
    FlySnapshotRefError, FlyVolume, FlyVolumeBinding, FlyVolumeBindingError, FlyVolumeId, FlyVolumeIdError,
    FlyWorkspaceVolume, FlyWorkspaceVolumeError, MAX_LEASES_LIMIT, MAX_VCPU_MILLIS, Machine, MachineId, MachineIdError,
    MachineSize, MachineSizeError, MachineState,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn nz(value: u32) -> Result<NonZeroU32, Box<dyn std::error::Error>> {
    NonZeroU32::new(value).ok_or_else(|| "zero".into())
}

fn region() -> Result<FlyRegion, Box<dyn std::error::Error>> {
    Ok(FlyRegion::new("iad")?)
}

fn binding() -> Result<FlyVolumeBinding, Box<dyn std::error::Error>> {
    let drive: DriveId = serde_json::from_value(json!({"tenant": "acme", "name": "data"}))?;
    Ok(FlyVolumeBinding::new(
        FlyVolumeId::new("vol_abc")?,
        drive,
        MountPath::new("/data")?,
    )?)
}

fn workspace_volume() -> Result<FlyWorkspaceVolume, Box<dyn std::error::Error>> {
    Ok(FlyWorkspaceVolume::new(
        nz(5)?,
        Duration::from_secs(1),
        Duration::from_secs(30),
    )?)
}

fn parts() -> Result<FlyConfigParts, Box<dyn std::error::Error>> {
    Ok(FlyConfigParts {
        image: "registry.example/img:1".into(),
        region: region()?,
        state_dir: "/tmp/state".into(),
        boot_timeout: Duration::from_secs(10),
        max_leases: 4,
        memory_mib: nz(512)?,
        cpus: nz(1)?,
        warm_pool_size: 0,
        volume: None,
        workspace_volume: None,
        egress_proxy: Some("http://proxy:3128".into()),
        public_host: Some("sandbox.example".into()),
        preview_network: FlyPreviewNetwork::Flycast,
    })
}

#[test]
fn api_token_accepts_and_redacts() -> TestResult {
    let token = FlyApiToken::new("secret")?;
    assert_eq!(format!("{token:?}"), "FlyApiToken(..)");
    let copy = token.clone();
    assert_eq!(format!("{copy:?}"), "FlyApiToken(..)");
    Ok(())
}

#[test]
fn api_token_refusals() -> TestResult {
    let empty = FlyApiToken::new("").err();
    assert!(matches!(empty, Some(FlyApiTokenError::Empty)));
    let bad = FlyApiToken::new("a\nb").err();
    assert!(matches!(bad, Some(FlyApiTokenError::NotAHeader)));
    assert_eq!(FlyApiTokenError::Empty.to_string(), "a Fly API token is required");
    assert_eq!(
        FlyApiTokenError::NotAHeader.to_string(),
        "the Fly API token cannot be sent in an HTTP header"
    );
    Ok(())
}

#[test]
fn app_name_rules() -> TestResult {
    let name = FlyAppName::new("my-app-1")?;
    assert_eq!(name.as_str(), "my-app-1");
    assert_eq!(name.to_string(), "my-app-1");
    assert!(FlyAppName::new("a".repeat(63)).is_ok());
    assert_eq!(FlyAppName::new(""), Err(FlyAppNameError));
    assert_eq!(FlyAppName::new("a".repeat(64)), Err(FlyAppNameError));
    assert_eq!(FlyAppName::new("bad_name"), Err(FlyAppNameError));
    assert_eq!(
        FlyAppNameError.to_string(),
        "a Fly app name is 1 to 63 letters, digits, or hyphens"
    );
    Ok(())
}

#[test]
fn region_rules() -> TestResult {
    let region = FlyRegion::new("us_east-1")?;
    assert_eq!(region.as_str(), "us_east-1");
    assert_eq!(region.to_string(), "us_east-1");
    assert_eq!(FlyRegion::new(""), Err(FlyRegionError));
    assert_eq!(FlyRegion::new("a.b"), Err(FlyRegionError));
    assert_eq!(
        FlyRegionError.to_string(),
        "a Fly region is letters, digits, underscores, or hyphens"
    );
    Ok(())
}

#[test]
fn endpoint_rules() -> TestResult {
    assert!(FlyEndpoint::parse("https://api.machines.dev")?.clone() == FlyEndpoint::parse("https://api.machines.dev")?);
    assert!(FlyEndpoint::parse("http://127.0.0.1:4280").is_ok());
    assert!(FlyEndpoint::parse("http://localhost:4280").is_ok());
    assert!(FlyEndpoint::parse("http://[::1]:4280").is_ok());
    assert_eq!(
        FlyEndpoint::parse("http://example.com"),
        Err(FlyEndpointError::InsecureScheme)
    );
    assert_eq!(
        FlyEndpoint::parse("ftp://example.com"),
        Err(FlyEndpointError::InsecureScheme)
    );
    assert_eq!(
        FlyEndpoint::parse("https://user:pw@example.com"),
        Err(FlyEndpointError::NotPlain)
    );
    assert_eq!(
        FlyEndpoint::parse("https://example.com?x=1"),
        Err(FlyEndpointError::NotPlain)
    );
    assert_eq!(
        FlyEndpoint::parse("https://example.com#frag"),
        Err(FlyEndpointError::NotPlain)
    );
    let unparsable = FlyEndpoint::parse("not a url");
    assert!(matches!(unparsable, Err(FlyEndpointError::Unparsable(_))));
    if let Err(error) = unparsable {
        assert!(!error.to_string().is_empty());
    }
    assert_eq!(
        FlyEndpointError::InsecureScheme.to_string(),
        "remote Fly API endpoints require HTTPS"
    );
    assert_eq!(
        FlyEndpointError::NotPlain.to_string(),
        "the endpoint must not carry credentials, a query, or a fragment"
    );
    Ok(())
}

#[test]
fn preview_network_default_and_variants() -> TestResult {
    assert_eq!(FlyPreviewNetwork::default(), FlyPreviewNetwork::Public);
    assert_ne!(FlyPreviewNetwork::Public, FlyPreviewNetwork::Flycast);
    Ok(())
}

#[test]
fn snapshot_deserializes() -> TestResult {
    let full: FlySnapshot =
        serde_json::from_value(json!({"id": "vs_1", "status": "created", "retention_days": 5, "size": 1024}))?;
    assert_eq!(full.id, "vs_1");
    assert_eq!(full.status.as_deref(), Some("created"));
    assert_eq!(full.retention_days, Some(5));
    assert_eq!(full.size, Some(1024));
    let minimal: FlySnapshot = serde_json::from_value(json!({"id": "vs_2"}))?;
    assert!(minimal.status.is_none() && minimal.retention_days.is_none() && minimal.size.is_none());
    assert!(format!("{:?}", minimal.clone()).contains("vs_2"));
    Ok(())
}

#[test]
fn snapshot_ref_round_trip() -> TestResult {
    let reference = FlySnapshotRef::new(region()?, "vs_abc-1", "img:1")?;
    assert_eq!(reference.snapshot_id(), "vs_abc-1");
    let encoded = reference.encode();
    assert!(encoded.starts_with("fly1.iad.vs_abc-1."));
    let decoded = FlySnapshotRef::decode(&encoded)?;
    assert_eq!(decoded, reference);
    decoded.ensure_restorable(&region()?, "img:1")?;
    Ok(())
}

#[test]
fn snapshot_ref_new_rejects_bad_id() -> TestResult {
    assert_eq!(
        FlySnapshotRef::new(region()?, "", "img"),
        Err(FlySnapshotRefError::InvalidSnapshotId)
    );
    assert_eq!(
        FlySnapshotRef::new(region()?, "a.b", "img"),
        Err(FlySnapshotRefError::InvalidSnapshotId)
    );
    Ok(())
}

#[test]
fn snapshot_ref_decode_rejections() -> TestResult {
    let hash = "0123456789abcdef";
    assert_eq!(
        FlySnapshotRef::decode("nope"),
        Err(FlySnapshotRefError::NotFlyReference)
    );
    assert_eq!(
        FlySnapshotRef::decode(&format!("fly2.iad.vs1.{hash}")),
        Err(FlySnapshotRefError::NotFlyReference)
    );
    assert_eq!(
        FlySnapshotRef::decode(&format!("fly1.iad..{hash}")),
        Err(FlySnapshotRefError::InvalidSnapshotId)
    );
    assert_eq!(
        FlySnapshotRef::decode("fly1.iad.vs1.short"),
        Err(FlySnapshotRefError::InvalidImageHash)
    );
    assert_eq!(
        FlySnapshotRef::decode("fly1.iad.vs1.zzzzzzzzzzzzzzzz"),
        Err(FlySnapshotRefError::InvalidImageHash)
    );
    assert_eq!(
        FlySnapshotRef::decode(&format!("fly1..vs1.{hash}")),
        Err(FlySnapshotRefError::Region(FlyRegionError))
    );
    Ok(())
}

#[test]
fn snapshot_ref_restorability() -> TestResult {
    let reference = FlySnapshotRef::new(region()?, "vs1", "img:1")?;
    let foreign_region = reference.ensure_restorable(&FlyRegion::new("ord")?, "img:1");
    assert_eq!(
        foreign_region,
        Err(FlySnapshotRefError::ForeignRegion {
            snapshot: "iad".into(),
            worker: "ord".into(),
        })
    );
    assert_eq!(
        reference.ensure_restorable(&region()?, "img:2"),
        Err(FlySnapshotRefError::ForeignImage)
    );
    Ok(())
}

#[test]
fn snapshot_ref_error_messages() -> TestResult {
    assert_eq!(
        FlySnapshotRefError::NotFlyReference.to_string(),
        "not a Fly snapshot reference"
    );
    assert_eq!(
        FlySnapshotRefError::InvalidSnapshotId.to_string(),
        "invalid snapshot id in a Fly snapshot reference"
    );
    assert_eq!(
        FlySnapshotRefError::Region(FlyRegionError).to_string(),
        "invalid region in a Fly snapshot reference: a Fly region is letters, digits, underscores, or hyphens"
    );
    assert_eq!(
        FlySnapshotRefError::InvalidImageHash.to_string(),
        "invalid image digest in a Fly snapshot reference"
    );
    assert_eq!(
        FlySnapshotRefError::ForeignRegion {
            snapshot: "iad".into(),
            worker: "ord".into(),
        }
        .to_string(),
        "Fly snapshot lives in region iad but this worker places Machines in ord"
    );
    assert_eq!(
        FlySnapshotRefError::ForeignImage.to_string(),
        "Fly snapshot was taken from a different image than this worker serves"
    );
    Ok(())
}

#[test]
fn volume_id_rules() -> TestResult {
    let id = FlyVolumeId::new("vol_abc123")?;
    assert_eq!(id.as_str(), "vol_abc123");
    assert_eq!(id.to_string(), "vol_abc123");
    assert_eq!(FlyVolumeId::new("abc"), Err(FlyVolumeIdError));
    assert_eq!(FlyVolumeId::new("vol_a-b"), Err(FlyVolumeIdError));
    assert_eq!(
        FlyVolumeId::new(format!("vol_{}", "a".repeat(253))),
        Err(FlyVolumeIdError)
    );
    assert!(FlyVolumeId::new(format!("vol_{}", "a".repeat(252))).is_ok());
    assert_eq!(
        FlyVolumeIdError.to_string(),
        "a Fly volume id is `vol_` followed by letters, digits, or underscores"
    );
    Ok(())
}

#[test]
fn volume_id_serde() -> TestResult {
    let id: FlyVolumeId = serde_json::from_value(json!("vol_x"))?;
    assert_eq!(serde_json::to_value(&id)?, json!("vol_x"));
    assert!(serde_json::from_value::<FlyVolumeId>(json!("bad")).is_err());
    assert_eq!(String::from(id), "vol_x");
    Ok(())
}

#[test]
fn volume_deserializes() -> TestResult {
    let full: FlyVolume = serde_json::from_value(json!({
        "id": "vol_1", "name": "ws", "state": "created", "region": "iad", "attached_machine_id": "m1"
    }))?;
    assert_eq!(full.id.as_str(), "vol_1");
    assert_eq!(full.name, "ws");
    assert_eq!(full.state, "created");
    assert_eq!(full.region.as_deref(), Some("iad"));
    assert_eq!(full.attached_machine_id.as_deref(), Some("m1"));
    let minimal: FlyVolume = serde_json::from_value(json!({"id": "vol_2"}))?;
    assert!(minimal.name.is_empty() && minimal.state.is_empty());
    assert!(minimal.region.is_none() && minimal.attached_machine_id.is_none());
    assert!(format!("{:?}", minimal.clone()).contains("vol_2"));
    Ok(())
}

#[test]
fn volume_binding_accessors() -> TestResult {
    let binding = binding()?;
    assert_eq!(binding.volume_id().as_str(), "vol_abc");
    assert_eq!(binding.provider_volume_id().as_str(), "vol_abc");
    assert_eq!(binding.drive_id().name.as_str(), "data");
    assert_eq!(binding.mount_path().as_str(), "/data");
    assert_eq!(binding.clone(), binding);
    Ok(())
}

#[test]
fn volume_binding_error_message() -> TestResult {
    let error = FlyVolumeBindingError::from(ProviderVolumeIdError::Invalid);
    assert_eq!(
        error.to_string(),
        "the Fly volume id is not a valid provider volume id: invalid provider volume ID"
    );
    Ok(())
}

#[test]
fn workspace_volume_rules() -> TestResult {
    let volume = workspace_volume()?;
    assert_eq!(volume.size_gb().get(), 5);
    assert_eq!(volume.snapshot_poll_interval(), Duration::from_secs(1));
    assert_eq!(volume.snapshot_timeout(), Duration::from_secs(30));
    assert_eq!(
        FlyWorkspaceVolume::new(nz(1)?, Duration::ZERO, Duration::from_secs(1)),
        Err(FlyWorkspaceVolumeError)
    );
    assert_eq!(
        FlyWorkspaceVolume::new(nz(1)?, Duration::from_secs(1), Duration::ZERO),
        Err(FlyWorkspaceVolumeError)
    );
    assert_eq!(
        FlyWorkspaceVolumeError.to_string(),
        "workspace volume snapshot timings must be non-zero"
    );
    Ok(())
}

#[test]
fn config_accepts_valid_parts() -> TestResult {
    let config = FlyConfig::new(parts()?)?;
    assert_eq!(config.image(), "registry.example/img:1");
    assert_eq!(config.region().as_str(), "iad");
    assert_eq!(config.state_dir(), std::path::Path::new("/tmp/state"));
    assert_eq!(config.max_leases(), 4);
    assert_eq!(config.warm_pool_size(), 0);
    assert!(config.volume().is_none());
    assert!(config.workspace_volume().is_none());
    assert!(format!("{:?}", config.clone()).contains("registry.example"));
    assert!(format!("{:?}", parts()?).contains("registry.example"));
    Ok(())
}

#[test]
fn config_accepts_volume_and_workspace_volume() -> TestResult {
    let mut with_volume = parts()?;
    with_volume.max_leases = 1;
    with_volume.volume = Some(binding()?);
    let config = FlyConfig::new(with_volume)?;
    assert!(config.volume().is_some());

    let mut with_workspace = parts()?;
    with_workspace.workspace_volume = Some(workspace_volume()?);
    let config = FlyConfig::new(with_workspace)?;
    assert!(config.workspace_volume().is_some());
    Ok(())
}

#[test]
fn config_rejects_inconsistent_parts() -> TestResult {
    let mut zero = parts()?;
    zero.max_leases = 0;
    assert_eq!(FlyConfig::new(zero).err(), Some(FlyConfigError::MaxLeases(0)));

    let mut high = parts()?;
    high.max_leases = MAX_LEASES_LIMIT + 1;
    assert_eq!(
        FlyConfig::new(high).err(),
        Some(FlyConfigError::MaxLeases(MAX_LEASES_LIMIT + 1))
    );

    let mut boot = parts()?;
    boot.boot_timeout = Duration::ZERO;
    assert_eq!(FlyConfig::new(boot).err(), Some(FlyConfigError::BootTimeout));

    let mut warm = parts()?;
    warm.warm_pool_size = 5;
    assert_eq!(
        FlyConfig::new(warm).err(),
        Some(FlyConfigError::WarmPoolExceedsCapacity)
    );

    let mut multi = parts()?;
    multi.volume = Some(binding()?);
    assert_eq!(
        FlyConfig::new(multi).err(),
        Some(FlyConfigError::VolumeNeedsSingleLease)
    );

    let mut warm_volume = parts()?;
    warm_volume.max_leases = 1;
    warm_volume.warm_pool_size = 1;
    warm_volume.volume = Some(binding()?);
    assert_eq!(
        FlyConfig::new(warm_volume).err(),
        Some(FlyConfigError::VolumeNeedsSingleLease)
    );

    let mut both = parts()?;
    both.max_leases = 1;
    both.volume = Some(binding()?);
    both.workspace_volume = Some(workspace_volume()?);
    assert_eq!(
        FlyConfig::new(both).err(),
        Some(FlyConfigError::WorkspaceVolumeWithOperatorVolume)
    );

    let mut pooled = parts()?;
    pooled.warm_pool_size = 1;
    pooled.workspace_volume = Some(workspace_volume()?);
    assert_eq!(
        FlyConfig::new(pooled).err(),
        Some(FlyConfigError::WorkspaceVolumeWithWarmPool)
    );
    Ok(())
}

#[test]
fn config_error_messages() -> TestResult {
    assert_eq!(
        FlyConfigError::MaxLeases(0).to_string(),
        "max leases must be from 1 to 128, not 0"
    );
    assert_eq!(
        FlyConfigError::BootTimeout.to_string(),
        "the boot timeout must be non-zero"
    );
    assert_eq!(
        FlyConfigError::WarmPoolExceedsCapacity.to_string(),
        "warm pool exceeds worker capacity"
    );
    assert_eq!(
        FlyConfigError::VolumeNeedsSingleLease.to_string(),
        "a configured persistent volume requires one lease and no warm pool"
    );
    assert_eq!(
        FlyConfigError::WorkspaceVolumeWithOperatorVolume.to_string(),
        "a workspace volume and an operator volume binding cannot be combined"
    );
    assert_eq!(
        FlyConfigError::WorkspaceVolumeWithWarmPool.to_string(),
        "a workspace volume needs a fresh Machine per lease, so it cannot be combined with a warm pool"
    );
    Ok(())
}

#[test]
fn machine_id_rules() -> TestResult {
    let id = MachineId::new("abc-123")?;
    assert_eq!(id.as_str(), "abc-123");
    assert_eq!(id.to_string(), "abc-123");
    assert_eq!(MachineId::new(""), Err(MachineIdError));
    assert_eq!(MachineId::new("a".repeat(65)), Err(MachineIdError));
    assert!(MachineId::new("a".repeat(64)).is_ok());
    assert_eq!(MachineId::new("a_b"), Err(MachineIdError));
    assert_eq!(
        MachineIdError.to_string(),
        "a Machine id is 1 to 64 letters, digits, or hyphens"
    );
    let parsed: MachineId = serde_json::from_value(json!("m-1"))?;
    assert_eq!(serde_json::to_value(&parsed)?, json!("m-1"));
    assert!(serde_json::from_value::<MachineId>(json!("a b")).is_err());
    assert_eq!(String::from(parsed), "m-1");
    Ok(())
}

#[test]
fn machine_size_fit() -> TestResult {
    let size = MachineSize::fit(None, nz(512)?, nz(1)?)?;
    assert_eq!((size.cpus(), size.memory_mib()), (1, 512));
    let rounded = MachineSize::fit(Some(1500), nz(300)?, nz(1)?)?;
    assert_eq!((rounded.cpus(), rounded.memory_mib()), (2, 512));
    let tiny = MachineSize::fit(Some(0), nz(256)?, nz(1)?)?;
    assert_eq!((tiny.cpus(), tiny.memory_mib()), (1, 256));
    let widened = MachineSize::fit(None, nz(5000)?, nz(1)?)?;
    assert_eq!((widened.cpus(), widened.memory_mib()), (4, 5120));
    let max = MachineSize::fit(Some(MAX_VCPU_MILLIS), nz(1024)?, nz(1)?)?;
    assert_eq!((max.cpus(), max.memory_mib()), (8, 2048));
    assert_eq!(max, max);
    Ok(())
}

#[test]
fn machine_size_refusals() -> TestResult {
    let too_many = MachineSize::fit(Some(9000), nz(512)?, nz(1)?);
    assert_eq!(
        too_many,
        Err(MachineSizeError {
            cpus: 9,
            memory_mib: 512
        })
    );
    let too_much_memory = MachineSize::fit(None, nz(20000)?, nz(1)?);
    assert_eq!(
        too_much_memory,
        Err(MachineSizeError {
            cpus: 1,
            memory_mib: 20000
        })
    );
    assert_eq!(
        MachineSizeError {
            cpus: 9,
            memory_mib: 512
        }
        .to_string(),
        "no Fly shared machine has 9 vCPUs and 512 MiB"
    );
    Ok(())
}

#[test]
fn machine_state_conversions() -> TestResult {
    let cases = [
        ("started", MachineState::Started),
        ("suspended", MachineState::Suspended),
        ("stopped", MachineState::Stopped),
        ("destroyed", MachineState::Destroyed),
        ("failed", MachineState::Failed),
        ("replacing", MachineState::Other("replacing".into())),
    ];
    for (text, state) in cases {
        assert_eq!(MachineState::from(text.to_string()), state);
        assert_eq!(String::from(state.clone()), text);
        assert_eq!(serde_json::to_value(&state)?, json!(text));
        assert_eq!(serde_json::from_value::<MachineState>(json!(text))?, state);
    }
    Ok(())
}

#[test]
fn machine_deserializes_and_reads_metadata() -> TestResult {
    let machine: Machine = serde_json::from_value(json!({
        "id": "m1",
        "state": "started",
        "config": {"metadata": {"lease": "abc", "count": 3}}
    }))?;
    assert_eq!(machine.id.as_str(), "m1");
    assert_eq!(machine.state, MachineState::Started);
    assert_eq!(machine.metadata("lease"), Some("abc"));
    assert_eq!(machine.metadata("count"), None);
    assert_eq!(machine.metadata("missing"), None);
    assert!(format!("{:?}", machine.clone()).contains("m1"));
    Ok(())
}

#[test]
fn exec_output_deserializes() -> TestResult {
    let full: ExecOutput = serde_json::from_value(json!({
        "exit_code": 2, "exit_signal": 9, "stdout": "out", "stderr": "err"
    }))?;
    assert_eq!((full.exit_code, full.exit_signal), (2, 9));
    assert_eq!((full.stdout.as_str(), full.stderr.as_str()), ("out", "err"));
    let minimal: ExecOutput = serde_json::from_value(json!({"exit_code": 0}))?;
    assert_eq!(minimal.exit_signal, 0);
    assert!(minimal.stdout.is_empty() && minimal.stderr.is_empty());
    assert!(format!("{:?}", minimal.clone()).contains("exit_code"));
    Ok(())
}
