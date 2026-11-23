use serde::{Deserialize, Serialize};

use crate::{DriveIdError, TenantId};

/// A tenant-scoped persistent drive name.
///
/// Deserialization does not validate `name` so stored drives keep parsing; [`DriveId::new`] and
/// [`DriveId::validate`] require 1 to [`DriveId::MAX_NAME_LEN`] bytes without `/`, `\`, or `..`.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct DriveId {
    pub tenant: TenantId,
    pub name: String,
}

impl DriveId {
    /// Longest drive name the wire accepts, in bytes.
    pub const MAX_NAME_LEN: usize = 64;

    pub fn new(tenant: TenantId, name: impl Into<String>) -> Result<Self, DriveIdError> {
        let drive_id = Self {
            tenant,
            name: name.into(),
        };
        drive_id.validate()?;
        Ok(drive_id)
    }

    pub fn validate(&self) -> Result<(), DriveIdError> {
        if self.name.is_empty() || self.name.len() > Self::MAX_NAME_LEN {
            return Err(DriveIdError::InvalidLength);
        }
        if self.name.contains('/') || self.name.contains('\\') || self.name.contains("..") {
            return Err(DriveIdError::InvalidCharacters);
        }
        Ok(())
    }
}
