use trogon_sandbox_contract::{SnapshotCaptureStatus, SnapshotReceipt};

use crate::ExportError;

/// Result of a release whose snapshot was exported and published. A tag failure does not make the snapshot "not
/// created": the digest is live and counted, so the capture status stays `Created` and only the tag step is
/// reported (and retried on its own, idempotently, never by rebuilding).
pub(crate) struct ReleaseOutcome {
    pub(crate) receipt: SnapshotReceipt,
    pub(crate) tag_failure: Option<ExportError>,
}

pub(crate) fn capture_status(result: &Result<ReleaseOutcome, ExportError>) -> SnapshotCaptureStatus {
    match result {
        Ok(outcome) => SnapshotCaptureStatus::Created {
            digest: outcome.receipt.digest.clone(),
            artifact_type: outcome.receipt.artifact_type,
        },
        Err(error) => SnapshotCaptureStatus::Failed {
            reason: error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests;
