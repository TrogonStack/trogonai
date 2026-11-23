use serde::{Deserialize, Serialize};

use crate::{SnapshotTagError, TenantId};

/// A tenant-scoped name that points at a snapshot digest.
///
/// Deserialization does not validate `name` so stored tags keep parsing; [`SnapshotTag::new`] and
/// [`SnapshotTag::validate`] require 1 to [`SnapshotTag::MAX_NAME_LEN`] bytes of ASCII letters,
/// digits, `.`, `_`, or `-`.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct SnapshotTag {
    pub tenant: TenantId,
    pub name: String,
}

impl SnapshotTag {
    /// Longest tag name the wire accepts, in bytes.
    pub const MAX_NAME_LEN: usize = 64;

    pub fn new(tenant: TenantId, name: impl Into<String>) -> Result<Self, SnapshotTagError> {
        let tag = Self {
            tenant,
            name: name.into(),
        };
        tag.validate()?;
        Ok(tag)
    }

    pub fn validate(&self) -> Result<(), SnapshotTagError> {
        if self.name.is_empty() || self.name.len() > Self::MAX_NAME_LEN {
            return Err(SnapshotTagError::InvalidLength);
        }
        if !self
            .name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-'))
        {
            return Err(SnapshotTagError::InvalidCharacters);
        }
        Ok(())
    }
}
