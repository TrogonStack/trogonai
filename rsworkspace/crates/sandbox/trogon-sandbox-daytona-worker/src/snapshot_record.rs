use serde::Deserialize;

use crate::snapshot_wire::SnapshotWire;
use crate::{SnapshotRecordError, SnapshotState};

/// The part of a Daytona snapshot the worker reads: what it is, who owns it, and which sandbox it was taken from.
#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "SnapshotWire")]
pub struct SnapshotRecord {
    pub id: String,
    pub name: String,
    pub state: SnapshotState,
    pub organization_id: Option<String>,
    pub source_sandbox_id: Option<String>,
    /// Shared by every organization, so never owned by one lease.
    pub general: bool,
    /// The size Daytona reports, rounded down.
    pub size: Option<u64>,
    pub error_reason: Option<String>,
}

impl TryFrom<SnapshotWire> for SnapshotRecord {
    type Error = SnapshotRecordError;

    fn try_from(wire: SnapshotWire) -> Result<Self, Self::Error> {
        if wire.id.is_empty() {
            return Err(SnapshotRecordError::MissingId);
        }
        if wire.name.is_empty() {
            return Err(SnapshotRecordError::MissingName);
        }
        let size = wire
            .size
            .filter(|size| size.is_finite() && *size >= 0.0)
            .map(|size| size.trunc() as u64);
        Ok(Self {
            id: wire.id,
            name: wire.name,
            state: wire.state,
            organization_id: wire.organization_id,
            source_sandbox_id: wire.source_sandbox_id,
            general: wire.general,
            size,
            error_reason: wire.error_reason,
        })
    }
}

#[cfg(test)]
mod tests;
