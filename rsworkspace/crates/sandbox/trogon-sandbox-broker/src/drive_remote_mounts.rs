use std::collections::HashMap;

use trogon_sandbox_contract::{
    DriveId, DriveMount, DriveMountMode, DriveRecord, MountAccess, RemoteMount, RemoteMountSource, S3BucketName,
};

/// Translates a sandbox's requested drive mounts into the remote mounts a worker already
/// knows how to mount, so drives ride the same generic S3 remote-mount path as any
/// explicitly requested remote mount.
///
/// `records` holds a record for every drive id in `drives`; entries with no matching record,
/// or whose storage is not an S3 bucket, are skipped because drive existence and ownership
/// are validated earlier in the lease request pipeline.
pub(crate) fn drives_to_remote_mounts(
    drives: &[DriveMount],
    records: &HashMap<DriveId, DriveRecord>,
) -> Vec<RemoteMount> {
    drives
        .iter()
        .filter_map(|mount| {
            let record = records.get(&mount.drive_id)?;
            let bucket = S3BucketName::new(record.storage.s3_bucket()?).ok()?;
            Some(RemoteMount {
                source: RemoteMountSource::S3 {
                    bucket,
                    prefix: drive_s3_prefix(&mount.drive_id),
                },
                mount_path: mount.mount_path.clone(),
                access: match mount.mode {
                    DriveMountMode::ReadOnly => MountAccess::ReadOnly,
                    DriveMountMode::ReadWrite => MountAccess::ReadWrite,
                },
            })
        })
        .collect()
}

/// Stable per-drive S3 key prefix, namespaced by tenant and drive name.
pub(crate) fn drive_s3_prefix(drive_id: &DriveId) -> String {
    format!("drives/{}/{}/", drive_id.tenant, drive_id.name)
}

#[cfg(test)]
mod tests;
