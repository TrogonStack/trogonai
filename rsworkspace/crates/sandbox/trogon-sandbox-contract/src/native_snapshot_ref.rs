use serde::{Deserialize, Serialize};

use crate::{NativeSnapshotIdError, ProviderKind};

/// A snapshot a provider keeps as a named object, with no digest and no registry location.
///
/// `id` is scoped to `provider`, so the pair is the identity, and only a worker of that provider
/// can restore it. Deserialization does not validate `id`; [`NativeSnapshotRef::validate`] does.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct NativeSnapshotRef {
    pub provider: ProviderKind,
    pub id: String,
    /// Unix seconds after which the provider may have dropped the snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}

impl NativeSnapshotRef {
    /// Longest provider snapshot id the wire accepts, in bytes.
    pub const MAX_ID_LEN: usize = 128;

    /// Requires an id of 1 to [`NativeSnapshotRef::MAX_ID_LEN`] bytes made of ASCII letters,
    /// digits, `-`, `_`, or `.`.
    pub fn validate(&self) -> Result<(), NativeSnapshotIdError> {
        if self.id.is_empty() || self.id.len() > Self::MAX_ID_LEN {
            return Err(NativeSnapshotIdError::InvalidLength { max: Self::MAX_ID_LEN });
        }
        if !self
            .id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
        {
            return Err(NativeSnapshotIdError::InvalidCharacters);
        }
        Ok(())
    }

    /// Whether the snapshot may be gone at `now_unix` seconds. A snapshot without an expiry never
    /// expires.
    pub fn is_expired_at(&self, now_unix: u64) -> bool {
        self.expires_at.is_some_and(|at| at <= now_unix)
    }
}
