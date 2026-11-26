use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{Digest, SnapshotTagName};

/// One flat entry in a [`SnapshotTagHistoryResponse`](crate::SnapshotTagHistoryResponse) listing.
///
/// Distinct from [`SnapshotTagAuditEvent::Set`](crate::SnapshotTagAuditEvent::Set): it is purpose-built for a history
/// listing, with no `prev` or `by`, not a full audit-trail event. `set_at` uses serde's `SystemTime` encoding.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotTagAuditEntry {
    pub name: SnapshotTagName,
    pub digest: Digest,
    pub set_at: SystemTime,
}
