use serde::{Deserialize, Serialize};

use crate::{DriveId, LeaseId};

/// Why a drive request was refused, tagged by `kind`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum DriveError {
    #[error("drive not found: {}:{}", .drive_id.tenant, .drive_id.name)]
    DriveNotFound { drive_id: DriveId },
    #[error("drive already exists: {}:{}", .drive_id.tenant, .drive_id.name)]
    DriveAlreadyExists { drive_id: DriveId },
    #[error("drive {}:{} is attached to lease {lease_id}", .drive_id.tenant, .drive_id.name)]
    DriveAttached { drive_id: DriveId, lease_id: LeaseId },
    #[error("requested drive size {requested_gib}GiB exceeds max {max_gib}GiB")]
    DriveSizeExceeded { requested_gib: u64, max_gib: u64 },
    #[error("too many drives: {count} exceeds max {max}")]
    TooManyDrives { count: usize, max: usize },
    #[error("duplicate drive mount_path '{mount_path}'")]
    DriveMountPathOverlap { mount_path: String },
    #[error("invalid drive mount_path '{mount_path}': {reason}")]
    InvalidDriveMountPath { mount_path: String, reason: String },
    #[error("drive storage backend error: {message}")]
    StorageBackend { message: String },
    /// The caller's credentials do not grant access to the drive's tenant.
    #[error("unauthorized: {reason}")]
    Unauthorized { reason: String },
}
