use std::str::FromStr;
use std::time::{Duration, UNIX_EPOCH};

use serde_json::json;
use trogon_sandbox_contract::{
    DriveError, DriveId, DriveIdError, DriveMount, DriveMountMode, DriveRecord, DriveStorage, LeaseId,
    NativeVolumeBinding, ProviderKind, ProviderVolumeId, ProviderVolumeIdError, ProviderVolumeRef, TenantId,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn drive_id(name: &str) -> Result<DriveId, Box<dyn std::error::Error>> {
    Ok(DriveId::new(TenantId::new("acme")?, name)?)
}

fn drive_mount(mount_path: &str) -> Result<DriveMount, Box<dyn std::error::Error>> {
    Ok(DriveMount {
        drive_id: drive_id("d0")?,
        mount_path: mount_path.to_owned(),
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
fn drive_id_accepts_valid_names() -> TestResult {
    let longest = "d".repeat(DriveId::MAX_NAME_LEN);
    for name in ["d0", "my drive.v2", "a.b", longest.as_str()] {
        let id = drive_id(name)?;
        assert_eq!(id.name, name);
        assert_eq!(id.validate(), Ok(()));
    }
    Ok(())
}

#[test]
fn drive_id_rejects_invalid_names() -> TestResult {
    let too_long = "d".repeat(DriveId::MAX_NAME_LEN + 1);
    for (name, expected) in [
        ("", DriveIdError::InvalidLength),
        (too_long.as_str(), DriveIdError::InvalidLength),
        ("a/b", DriveIdError::InvalidCharacters),
        ("a\\b", DriveIdError::InvalidCharacters),
        ("..", DriveIdError::InvalidCharacters),
        ("a..b", DriveIdError::InvalidCharacters),
    ] {
        assert_eq!(
            DriveId::new(TenantId::new("acme")?, name).err(),
            Some(expected),
            "{name:?}"
        );
    }
    assert_eq!(
        DriveIdError::InvalidLength.to_string(),
        "drive name must be 1-64 characters"
    );
    assert_eq!(
        DriveIdError::InvalidCharacters.to_string(),
        "drive name must not contain '/', '\\\\', or '..'"
    );
    Ok(())
}

#[test]
fn drive_id_deserializes_without_validating() -> TestResult {
    let id = drive_id("d0")?;
    let wire = serde_json::to_value(&id)?;
    assert_eq!(wire, json!({ "tenant": "acme", "name": "d0" }));
    assert_eq!(serde_json::from_value::<DriveId>(wire)?, id);

    let stored: DriveId = serde_json::from_value(json!({ "tenant": "acme", "name": "a/b" }))?;
    assert_eq!(stored.validate(), Err(DriveIdError::InvalidCharacters));
    Ok(())
}

#[test]
fn drive_mount_validate_accepts_an_absolute_path() -> TestResult {
    for path in ["/mnt/d0", "/mnt/..d0", "/mnt/d0/"] {
        assert_eq!(drive_mount(path)?.validate(), Ok(()), "{path}");
    }
    Ok(())
}

#[test]
fn drive_mount_validate_rejects_unsafe_paths() -> TestResult {
    for (path, reason) in [
        ("relative/path", "must be absolute"),
        ("", "must be absolute"),
        ("/", "must not be '/'"),
        ("/mnt/../etc", "must not contain '..' components"),
        ("/..", "must not contain '..' components"),
    ] {
        assert_eq!(
            drive_mount(path)?.validate(),
            Err(DriveError::InvalidDriveMountPath {
                mount_path: path.to_owned(),
                reason: reason.to_owned(),
            }),
            "{path}"
        );
    }
    Ok(())
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
