/// What the writer did with a record it accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Disposition {
    /// Durably stored and acknowledged.
    Stored,
    /// A refusal's record is in the bounded retry buffer and will be retried under
    /// the same message id. The refusal stands either way.
    Retrying,
    /// A best-effort record is in the bounded queue and will be published
    /// asynchronously.
    Queued,
    /// The record was not kept and the drop was counted. For a refusal's record
    /// this is the one place a must-audit record is lost.
    Dropped,
}
