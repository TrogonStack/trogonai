/// Restricts [`ExecutionSnapshots`](super::ExecutionSnapshots) to the two configurations this
/// crate ships.
///
/// The trait is public because it appears in [`CommandExecution::execute`](super::CommandExecution::execute)'s
/// signature, not because a third configuration is supported: it is the crate's internal execution
/// seam and its phases move as the runtime grows.
pub trait Sealed {}
