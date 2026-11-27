/// Daytona described a snapshot the worker cannot use.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SnapshotRecordError {
    #[error("Daytona returned a snapshot without an id")]
    MissingId,
    #[error("Daytona returned a snapshot without a name")]
    MissingName,
}
