use std::collections::HashMap;
use std::error::Error;
use std::time::SystemTime;

use trogon_sandbox_contract::{
    DriveId, DriveMount, DriveMountMode, DriveName, DriveRecord, DriveStorage, MountAccess, MountPath,
    RemoteMountSource, TenantId,
};

use super::{drive_s3_prefix, drives_to_remote_mounts};

fn record(id: &DriveId, bucket: &str) -> DriveRecord {
    DriveRecord {
        id: id.clone(),
        tenant: id.tenant.clone(),
        size_gib: DriveRecord::DEFAULT_SIZE_GIB,
        created_at: SystemTime::UNIX_EPOCH,
        attached_lease: None,
        storage: DriveStorage::S3 {
            bucket: bucket.to_owned(),
        },
    }
}

fn drive(name: &str) -> Result<DriveId, Box<dyn Error>> {
    Ok(DriveId {
        tenant: TenantId::new("acme")?,
        name: DriveName::new(name)?,
    })
}

fn mount(id: &DriveId, path: &str, mode: DriveMountMode) -> Result<DriveMount, Box<dyn Error>> {
    Ok(DriveMount {
        drive_id: id.clone(),
        mount_path: MountPath::new(path)?,
        mode,
    })
}

#[test]
fn read_write_and_read_only_modes_map_to_mount_access() -> Result<(), Box<dyn Error>> {
    let data = drive("data")?;
    let cache = drive("cache")?;
    let records = HashMap::from([
        (data.clone(), record(&data, "bucket-one")),
        (cache.clone(), record(&cache, "bucket-one")),
    ]);
    let drives = [
        mount(&data, "/mnt/data", DriveMountMode::ReadWrite)?,
        mount(&cache, "/mnt/cache", DriveMountMode::ReadOnly)?,
    ];
    let mounts = drives_to_remote_mounts(&drives, &records);
    assert_eq!(mounts.len(), 2);
    assert_eq!(mounts[0].access, MountAccess::ReadWrite);
    assert_eq!(mounts[1].access, MountAccess::ReadOnly);
    let RemoteMountSource::S3 { bucket, prefix } = &mounts[0].source;
    assert_eq!(bucket.as_str(), "bucket-one");
    assert_eq!(prefix, &drive_s3_prefix(&data));
    assert_eq!(prefix, "drives/acme/data/");
    Ok(())
}

#[test]
fn drives_without_a_usable_record_are_skipped() -> Result<(), Box<dyn Error>> {
    let known = drive("known")?;
    let missing = drive("missing")?;
    let invalid = drive("invalid")?;
    let records = HashMap::from([
        (known.clone(), record(&known, "bucket-one")),
        (invalid.clone(), record(&invalid, "UPPER")),
    ]);
    let drives = [
        mount(&missing, "/mnt/a", DriveMountMode::ReadWrite)?,
        mount(&invalid, "/mnt/b", DriveMountMode::ReadWrite)?,
        mount(&known, "/mnt/c", DriveMountMode::ReadWrite)?,
    ];
    let mounts = drives_to_remote_mounts(&drives, &records);
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0].mount_path.as_str(), "/mnt/c");
    Ok(())
}
