use std::collections::HashMap;

use trogon_sandbox_contract::{
    DriveId, DriveRecord, LeaseError, NetworkPolicy, SandboxImage, SandboxSpec, SandboxSpecParts,
};

use crate::drive_remote_mounts::drives_to_remote_mounts;

/// Rebuilds `spec` after `edit` changed its parts, which checks every spec rule again.
fn edit_spec(spec: &SandboxSpec, edit: impl FnOnce(&mut SandboxSpecParts)) -> Result<SandboxSpec, LeaseError> {
    let mut parts = spec.clone().into_parts();
    edit(&mut parts);
    SandboxSpec::new(parts).map_err(|error| LeaseError::InvalidSpec {
        reason: error.to_string(),
    })
}

/// `spec` booting from `image` instead.
pub(crate) fn with_image(spec: &SandboxSpec, image: SandboxImage) -> Result<SandboxSpec, LeaseError> {
    edit_spec(spec, |parts| parts.image = image)
}

/// `spec` with `network_policy` as its policy.
pub(crate) fn with_network_policy(
    spec: &SandboxSpec,
    network_policy: NetworkPolicy,
) -> Result<SandboxSpec, LeaseError> {
    edit_spec(spec, |parts| parts.network_policy = Some(network_policy))
}

/// `spec` with a remote mount appended for every drive that `records` backs with a bucket.
pub(crate) fn with_remote_drive_mounts(
    spec: &SandboxSpec,
    records: &HashMap<DriveId, DriveRecord>,
) -> Result<SandboxSpec, LeaseError> {
    let mounts = drives_to_remote_mounts(spec.drives(), records);
    edit_spec(spec, |parts| parts.mounts.extend(mounts))
}

/// `spec` keeping only the drives that `records` backs with a bucket.
pub(crate) fn with_bucket_backed_drives(
    spec: &SandboxSpec,
    records: &HashMap<DriveId, DriveRecord>,
) -> Result<SandboxSpec, LeaseError> {
    edit_spec(spec, |parts| {
        parts.drives.retain(|mount| {
            records
                .get(&mount.drive_id)
                .is_some_and(|record| record.storage.s3_bucket().is_some())
        });
    })
}
