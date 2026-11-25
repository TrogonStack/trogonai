use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{Digest, ServiceAccountId, SnapshotTag};

/// A change to a snapshot tag, tagged by `event` as `set` or `cleared`.
///
/// `at` uses serde's `SystemTime` encoding, an object of `secs_since_epoch` and
/// `nanos_since_epoch`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum SnapshotTagAuditEvent {
    /// The tag now points at `next`. `prev` is `null` when the tag is new.
    Set {
        tag: SnapshotTag,
        prev: Option<Digest>,
        next: Digest,
        by: ServiceAccountId,
        at: SystemTime,
    },
    /// The tag no longer points at `prev`.
    Cleared {
        tag: SnapshotTag,
        prev: Digest,
        by: ServiceAccountId,
        at: SystemTime,
    },
}
