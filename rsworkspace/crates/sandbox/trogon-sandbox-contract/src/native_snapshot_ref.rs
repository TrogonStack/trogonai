use serde::{Deserialize, Serialize};

use crate::{NativeSnapshotId, ProviderKind};

/// A snapshot a provider keeps as a named object, with no digest and no registry location.
///
/// `id` is scoped to `provider`, so the pair is the identity, and only a worker of that provider
/// can restore it.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct NativeSnapshotRef {
    pub provider: ProviderKind,
    pub id: NativeSnapshotId,
    /// Unix seconds after which the provider may have dropped the snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}

impl NativeSnapshotRef {
    /// Whether the snapshot may be gone at `now_unix` seconds. A snapshot without an expiry never
    /// expires.
    pub fn is_expired_at(&self, now_unix: u64) -> bool {
        self.expires_at.is_some_and(|at| at <= now_unix)
    }
}
