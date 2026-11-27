/// What [`crate::ExecutionTracker::status`] knows about an execution, for
/// callers that only need to tell a live one from a finished or unknown one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExecutionStatus {
    Unknown,
    Running,
    Finished,
}
