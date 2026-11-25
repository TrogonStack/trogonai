use std::str::FromStr;
use std::time::{Duration, UNIX_EPOCH};

use serde_json::json;
use trogon_sandbox_contract::{
    DriveError, DriveId, DriveMount, DriveMountMode, DriveName, DriveNameError, DriveRecord, DriveStorage, LeaseId,
    MountPath, MountPathError, NativeVolumeBinding, ProviderKind, ProviderVolumeId, ProviderVolumeIdError,
    ProviderVolumeRef, TenantId,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn drive_id(name: &str) -> Result<DriveId, Box<dyn std::error::Error>> {
    Ok(DriveId {
        tenant: TenantId::new("acme")?,
        name: DriveName::new(name)?,
    })
}

fn drive_mount(mount_path: &str) -> Result<DriveMount, Box<dyn std::error::Error>> {
    Ok(DriveMount {
        drive_id: drive_id("d0")?,
        mount_path: MountPath::new(mount_path)?,
        mode: DriveMountMode::default(),
    })
}

fn provider_volume() -> Result<ProviderVolumeRef, Box<dyn std::error::Error>> {
    Ok(ProviderVolumeRef {
        provider: ProviderKind::Fly,
        volume_id: ProviderVolumeId::new("vol_123")?,
    })
}

#[test]
fn drive_name_accepts_valid_names() -> TestResult {
    let longest = "d".repeat(DriveName::MAX_LEN);
    for value in ["d0", "my drive.v2", "a.b", longest.as_str()] {
        let name = DriveName::new(value)?;
        assert_eq!(name.as_str(), value);
        assert_eq!(name.to_string(), value);
        assert_eq!(DriveName::from_str(value)?, name);
        assert_eq!(DriveName::try_from(value.to_owned())?, name);
        assert_eq!(serde_json::to_value(&name)?, json!(value));
        assert_eq!(serde_json::from_value::<DriveName>(json!(value))?, name);
        assert_eq!(String::from(name), value);
    }
    Ok(())
}

#[test]
fn drive_name_rejects_invalid_names_at_construction_and_deserialization() {
    let too_long = "d".repeat(DriveName::MAX_LEN + 1);
    for (value, expected) in [
        ("", DriveNameError::InvalidLength),
        (too_long.as_str(), DriveNameError::InvalidLength),
        ("a/b", DriveNameError::InvalidCharacters),
        ("a\\b", DriveNameError::InvalidCharacters),
        ("..", DriveNameError::InvalidCharacters),
        ("a..b", DriveNameError::InvalidCharacters),
    ] {
        assert_eq!(DriveName::new(value), Err(expected), "{value:?}");
        let error = serde_json::from_value::<DriveName>(json!(value)).err();
        assert_eq!(
            error.map(|error| error.to_string()),
            Some(expected.to_string()),
            "{value:?}"
        );
    }
    assert_eq!(
        DriveNameError::InvalidLength.to_string(),
        "drive name must be 1-64 characters"
    );
    assert_eq!(
        DriveNameError::InvalidCharacters.to_string(),
        "drive name must not contain '/', '\\\\', or '..'"
    );
}

#[test]
fn drive_id_roundtrips_and_rejects_an_invalid_name() -> TestResult {
    let id = drive_id("d0")?;
    let wire = serde_json::to_value(&id)?;
    assert_eq!(wire, json!({ "tenant": "acme", "name": "d0" }));
    assert_eq!(serde_json::from_value::<DriveId>(wire)?, id);

    let error = serde_json::from_value::<DriveId>(json!({ "tenant": "acme", "name": "a/b" })).err();
    assert_eq!(
        error.map(|error| error.to_string()),
        Some(DriveNameError::InvalidCharacters.to_string())
    );
    Ok(())
}

#[test]
fn mount_path_accepts_absolute_paths() -> TestResult {
    for value in ["/mnt/d0", "/mnt/..d0", "/mnt/d0/", "/mnt/./d0", "/.d0"] {
        let path = MountPath::new(value)?;
        assert_eq!(path.as_str(), value);
        assert_eq!(path.to_string(), value);
        assert_eq!(MountPath::from_str(value)?, path);
        assert_eq!(MountPath::try_from(value.to_owned())?, path);
        assert_eq!(serde_json::to_value(&path)?, json!(value));
        assert_eq!(serde_json::from_value::<MountPath>(json!(value))?, path);
        assert_eq!(String::from(path), value);
    }
    Ok(())
}

#[test]
fn mount_path_rejects_unsafe_paths_at_construction_and_deserialization() {
    for (value, expected) in [
        ("relative/path", MountPathError::NotAbsolute),
        ("", MountPathError::NotAbsolute),
        ("/", MountPathError::Root),
        ("/.", MountPathError::Root),
        ("/./", MountPathError::Root),
        ("///", MountPathError::Root),
        ("/mnt/../etc", MountPathError::ParentComponent),
        ("/..", MountPathError::ParentComponent),
    ] {
        assert_eq!(MountPath::new(value), Err(expected), "{value:?}");
        let error = serde_json::from_value::<MountPath>(json!(value)).err();
        assert_eq!(
            error.map(|error| error.to_string()),
            Some(expected.to_string()),
            "{value:?}"
        );
    }
    assert_eq!(MountPathError::NotAbsolute.to_string(), "mount path must be absolute");
    assert_eq!(MountPathError::Root.to_string(), "mount path must not resolve to '/'");
    assert_eq!(
        MountPathError::ParentComponent.to_string(),
        "mount path must not contain '..' components"
    );
}

#[test]
fn drive_mount_rejects_an_unsafe_mount_path() {
    let error = serde_json::from_value::<DriveMount>(json!({
        "drive_id": { "tenant": "acme", "name": "d0" },
        "mount_path": "/",
    }))
    .err();
    assert_eq!(
        error.map(|error| error.to_string()),
        Some(MountPathError::Root.to_string())
    );
}

#[test]
fn drive_mount_mode_defaults_to_read_write_when_omitted() -> TestResult {
    assert_eq!(DriveMountMode::default(), DriveMountMode::ReadWrite);
    assert_eq!(serde_json::to_value(DriveMountMode::ReadWrite)?, json!("read_write"));
    assert_eq!(serde_json::to_value(DriveMountMode::ReadOnly)?, json!("read_only"));

    let mount: DriveMount = serde_json::from_value(json!({
        "drive_id": { "tenant": "acme", "name": "d0" },
        "mount_path": "/mnt/d0",
    }))?;
    assert_eq!(mount, drive_mount("/mnt/d0")?);

    let read_only = DriveMount {
        mode: DriveMountMode::ReadOnly,
        ..drive_mount("/mnt/d0")?
    };
    let wire = serde_json::to_value(&read_only)?;
    assert_eq!(
        wire,
        json!({
            "drive_id": { "tenant": "acme", "name": "d0" },
            "mount_path": "/mnt/d0",
            "mode": "read_only",
        })
    );
    assert_eq!(serde_json::from_value::<DriveMount>(wire)?, read_only);
    Ok(())
}

#[test]
fn provider_volume_id_accepts_printable_values() -> TestResult {
    let longest = "v".repeat(ProviderVolumeId::MAX_LEN);
    for value in ["vol_123", "projects/p/volumes/v:1", longest.as_str()] {
        let id = ProviderVolumeId::new(value)?;
        assert_eq!(id.as_str(), value);
        assert_eq!(id.to_string(), value);
        assert_eq!(ProviderVolumeId::from_str(value)?, id);
        assert_eq!(ProviderVolumeId::try_from(value.to_owned())?, id);
        assert_eq!(serde_json::to_value(&id)?, json!(value));
        assert_eq!(serde_json::from_value::<ProviderVolumeId>(json!(value))?, id);
        assert_eq!(String::from(id), value);
    }
    Ok(())
}

#[test]
fn provider_volume_id_rejects_empty_long_blank_or_control_values() {
    let too_long = "v".repeat(ProviderVolumeId::MAX_LEN + 1);
    for value in [
        "",
        too_long.as_str(),
        "has space",
        "tab\there",
        "bell\u{7}",
        "nbsp\u{a0}",
    ] {
        assert_eq!(
            ProviderVolumeId::new(value),
            Err(ProviderVolumeIdError::Invalid),
            "{value:?}"
        );
        let error = serde_json::from_value::<ProviderVolumeId>(json!(value)).err();
        assert_eq!(
            error.map(|error| error.to_string()),
            Some("invalid provider volume ID".to_owned()),
            "{value:?}"
        );
    }
}

#[test]
fn native_volume_binding_roundtrips() -> TestResult {
    let binding = NativeVolumeBinding {
        mount: drive_mount("/mnt/d0")?,
        storage: provider_volume()?,
    };
    let wire = serde_json::to_value(&binding)?;
    assert_eq!(
        wire,
        json!({
            "mount": {
                "drive_id": { "tenant": "acme", "name": "d0" },
                "mount_path": "/mnt/d0",
                "mode": "read_write",
            },
            "storage": { "provider": "fly", "volume_id": "vol_123" },
        })
    );
    assert_eq!(serde_json::from_value::<NativeVolumeBinding>(wire)?, binding);
    Ok(())
}

#[test]
fn drive_storage_exposes_its_backing() -> TestResult {
    let s3 = DriveStorage::S3 {
        bucket: "drives".to_owned(),
    };
    assert_eq!(s3.s3_bucket(), Some("drives"));
    assert_eq!(s3.provider_volume(), None);

    let volume = DriveStorage::ProviderVolume {
        provider_volume: provider_volume()?,
    };
    assert_eq!(volume.s3_bucket(), None);
    assert_eq!(volume.provider_volume(), Some(&provider_volume()?));
    Ok(())
}

#[test]
fn drive_storage_is_untagged_and_rejects_unknown_fields() -> TestResult {
    let s3 = DriveStorage::S3 {
        bucket: "drives".to_owned(),
    };
    let wire = serde_json::to_value(&s3)?;
    assert_eq!(wire, json!({ "bucket": "drives" }));
    assert_eq!(serde_json::from_value::<DriveStorage>(wire)?, s3);

    let volume = DriveStorage::ProviderVolume {
        provider_volume: provider_volume()?,
    };
    let wire = serde_json::to_value(&volume)?;
    assert_eq!(
        wire,
        json!({ "provider_volume": { "provider": "fly", "volume_id": "vol_123" } })
    );
    assert_eq!(serde_json::from_value::<DriveStorage>(wire)?, volume);

    assert!(serde_json::from_value::<DriveStorage>(json!({ "bucket": "drives", "region": "x" })).is_err());
    assert!(serde_json::from_value::<DriveStorage>(json!({})).is_err());
    Ok(())
}

#[test]
fn drive_record_flattens_its_storage() -> TestResult {
    let record = DriveRecord {
        id: drive_id("d0")?,
        tenant: TenantId::new("acme")?,
        size_gib: DriveRecord::DEFAULT_SIZE_GIB,
        created_at: UNIX_EPOCH + Duration::new(1_700_000_000, 5),
        attached_lease: None,
        storage: DriveStorage::S3 {
            bucket: "drives".to_owned(),
        },
    };
    let wire = serde_json::to_value(&record)?;
    assert_eq!(
        wire,
        json!({
            "id": { "tenant": "acme", "name": "d0" },
            "tenant": "acme",
            "size_gib": 100,
            "created_at": { "secs_since_epoch": 1_700_000_000u64, "nanos_since_epoch": 5 },
            "attached_lease": null,
            "bucket": "drives",
        })
    );
    assert_eq!(serde_json::from_value::<DriveRecord>(wire)?, record);

    let attached = DriveRecord {
        attached_lease: Some(LeaseId::new("lease-1")),
        storage: DriveStorage::ProviderVolume {
            provider_volume: provider_volume()?,
        },
        ..record
    };
    let wire = serde_json::to_value(&attached)?;
    assert_eq!(wire["attached_lease"], json!("lease-1"));
    assert_eq!(
        wire["provider_volume"],
        json!({ "provider": "fly", "volume_id": "vol_123" })
    );
    assert_eq!(serde_json::from_value::<DriveRecord>(wire)?, attached);
    Ok(())
}

#[test]
fn drive_record_rejects_an_unknown_top_level_field() {
    let wire = json!({
        "id": { "tenant": "acme", "name": "d0" },
        "tenant": "acme",
        "size_gib": 100,
        "created_at": { "secs_since_epoch": 0, "nanos_since_epoch": 0 },
        "attached_lease": null,
        "bucket": "drives",
        "region": "x",
    });
    assert!(serde_json::from_value::<DriveRecord>(wire).is_err());
}

#[test]
fn drive_size_limits_keep_their_values() {
    assert_eq!(DriveRecord::DEFAULT_SIZE_GIB, 100);
    assert_eq!(DriveRecord::MAX_SIZE_GIB, 1024);
}

#[test]
fn drive_errors_are_tagged_by_kind_and_keep_their_messages() -> TestResult {
    let id = drive_id("d0")?;
    let cases = [
        (
            DriveError::DriveNotFound { drive_id: id.clone() },
            json!({ "kind": "drive_not_found", "drive_id": { "tenant": "acme", "name": "d0" } }),
            "drive not found: acme:d0",
        ),
        (
            DriveError::DriveAlreadyExists { drive_id: id.clone() },
            json!({ "kind": "drive_already_exists", "drive_id": { "tenant": "acme", "name": "d0" } }),
            "drive already exists: acme:d0",
        ),
        (
            DriveError::DriveAttached {
                drive_id: id,
                lease_id: LeaseId::new("lease-1"),
            },
            json!({
                "kind": "drive_attached",
                "drive_id": { "tenant": "acme", "name": "d0" },
                "lease_id": "lease-1",
            }),
            "drive acme:d0 is attached to lease lease-1",
        ),
        (
            DriveError::DriveSizeExceeded {
                requested_gib: 2048,
                max_gib: DriveRecord::MAX_SIZE_GIB,
            },
            json!({ "kind": "drive_size_exceeded", "requested_gib": 2048, "max_gib": 1024 }),
            "requested drive size 2048GiB exceeds max 1024GiB",
        ),
        (
            DriveError::TooManyDrives { count: 9, max: 8 },
            json!({ "kind": "too_many_drives", "count": 9, "max": 8 }),
            "too many drives: 9 exceeds max 8",
        ),
        (
            DriveError::DriveMountPathOverlap {
                mount_path: "/mnt/d0".to_owned(),
            },
            json!({ "kind": "drive_mount_path_overlap", "mount_path": "/mnt/d0" }),
            "duplicate drive mount_path '/mnt/d0'",
        ),
        (
            DriveError::InvalidDriveMountPath {
                mount_path: "/".to_owned(),
                reason: "must not be '/'".to_owned(),
            },
            json!({ "kind": "invalid_drive_mount_path", "mount_path": "/", "reason": "must not be '/'" }),
            "invalid drive mount_path '/': must not be '/'",
        ),
        (
            DriveError::StorageBackend {
                message: "timeout".to_owned(),
            },
            json!({ "kind": "storage_backend", "message": "timeout" }),
            "drive storage backend error: timeout",
        ),
        (
            DriveError::Unauthorized {
                reason: "wrong tenant".to_owned(),
            },
            json!({ "kind": "unauthorized", "reason": "wrong tenant" }),
            "unauthorized: wrong tenant",
        ),
    ];
    for (error, wire, message) in cases {
        assert_eq!(serde_json::to_value(&error)?, wire, "{message}");
        assert_eq!(serde_json::from_value::<DriveError>(wire)?, error, "{message}");
        assert_eq!(error.to_string(), message);
    }
    Ok(())
}
