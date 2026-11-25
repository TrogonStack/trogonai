use serde::{Deserialize, Serialize};

/// Why snapshot accounting refused to record a publication.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotAccountingRefusal {
    LegacyUnconfirmed,
    UnknownGeneration,
    OwnershipMismatch,
    MetadataMismatch,
    Capacity,
    QuotaExceeded,
    Pending,
    Backend,
}
