use serde::{Deserialize, Serialize};

use crate::{NativeSnapshotReceipt, SnapshotExportError, SnapshotReceipt};

/// The result of a snapshot export, tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum SnapshotExportOutcome {
    Ok(SnapshotReceipt),
    /// The provider kept the snapshot itself. Sent only for a request that named a native snapshot.
    Native(NativeSnapshotReceipt),
    Err(SnapshotExportError),
}
