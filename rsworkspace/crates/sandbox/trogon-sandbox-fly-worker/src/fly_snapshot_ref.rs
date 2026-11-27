//! Wire form of a Fly volume snapshot as a `NativeSnapshotRef` id.
//!
//! The ref carries one opaque id limited to ASCII letters, digits, `-`, `_` and `.`, but a Fly restore needs
//! the snapshot id, the region it lives in and the image the Machine was booted from. The id packs all three as
//! `fly1.<region>.<snapshot>.<image digest prefix>`. The image is stored as a digest of its reference because an
//! image reference is not a legal id character set and the worker serves exactly one configured image, so a
//! restore only has to prove the image is the same one.

use sha2::{Digest, Sha256};

use crate::constants::{IMAGE_HASH_HEX_LEN, VERSION};
use crate::{FlyRegion, FlySnapshotRefError};

/// The region, snapshot id and image digest a Fly snapshot reference packs.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FlySnapshotRef {
    region: FlyRegion,
    snapshot_id: String,
    image_hash: String,
}

fn image_hash(image: &str) -> String {
    Sha256::digest(image.as_bytes())
        .iter()
        .take(IMAGE_HASH_HEX_LEN / 2)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn is_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

impl FlySnapshotRef {
    pub fn new(region: FlyRegion, snapshot_id: &str, image: &str) -> Result<Self, FlySnapshotRefError> {
        if !is_token(snapshot_id) {
            return Err(FlySnapshotRefError::InvalidSnapshotId);
        }
        Ok(Self {
            region,
            snapshot_id: snapshot_id.into(),
            image_hash: image_hash(image),
        })
    }

    pub fn snapshot_id(&self) -> &str {
        &self.snapshot_id
    }

    pub fn encode(&self) -> String {
        format!("{VERSION}.{}.{}.{}", self.region, self.snapshot_id, self.image_hash)
    }

    pub fn decode(id: &str) -> Result<Self, FlySnapshotRefError> {
        let parts: Vec<_> = id.split('.').collect();
        let [VERSION, region, snapshot_id, image_hash] = parts.as_slice() else {
            return Err(FlySnapshotRefError::NotFlyReference);
        };
        if !is_token(snapshot_id) {
            return Err(FlySnapshotRefError::InvalidSnapshotId);
        }
        let well_formed =
            image_hash.len() == IMAGE_HASH_HEX_LEN && image_hash.bytes().all(|byte| byte.is_ascii_hexdigit());
        if !well_formed {
            return Err(FlySnapshotRefError::InvalidImageHash);
        }
        Ok(Self {
            region: FlyRegion::new(*region).map_err(FlySnapshotRefError::Region)?,
            snapshot_id: (*snapshot_id).into(),
            image_hash: (*image_hash).into(),
        })
    }

    /// A restore boots the worker's own image in its own region, so a reference taken elsewhere cannot be honored.
    pub fn ensure_restorable(&self, region: &FlyRegion, image: &str) -> Result<(), FlySnapshotRefError> {
        if self.region != *region {
            return Err(FlySnapshotRefError::ForeignRegion {
                snapshot: self.region.to_string(),
                worker: region.to_string(),
            });
        }
        if self.image_hash != image_hash(image) {
            return Err(FlySnapshotRefError::ForeignImage);
        }
        Ok(())
    }
}
