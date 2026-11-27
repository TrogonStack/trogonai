/// Why a released snapshot was not exported, secured, or published.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("missing worker_id on lease release event")]
    MissingWorkerId,
    #[error("permanent export failure: {0}")]
    Permanent(String),
    #[error("export failed after retries: {0}")]
    Exhausted(String),
    #[error("snapshot export outcome unknown: {0}")]
    OutcomeUnknown(String),
    #[error("scan gate rejected publication: {0}")]
    ScanRejected(String),
    #[error("snapshot published but tag not moved: {0}")]
    TagNotMoved(String),
}

impl ExportError {
    /// The `reason` label this failure is counted under.
    pub fn reason_label(&self) -> &'static str {
        match self {
            Self::MissingWorkerId => "missing_worker_id",
            Self::Permanent(_) => "permanent",
            Self::Exhausted(_) => "exhausted_retries",
            Self::OutcomeUnknown(_) => "outcome_unknown",
            Self::ScanRejected(_) => "scan_rejected",
            Self::TagNotMoved(_) => "tag_not_moved",
        }
    }
}
