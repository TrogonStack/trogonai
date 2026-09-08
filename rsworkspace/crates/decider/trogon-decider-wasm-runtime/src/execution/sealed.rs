/// Restricts [`WasmExecutionSnapshots`](super::WasmExecutionSnapshots) to the two
/// configurations this crate ships.
///
/// The trait is public because it appears in
/// [`WasmCommandExecution::execute`](super::WasmCommandExecution::execute)'s signature, not because
/// a third configuration is supported: it is the crate's internal execution seam and its phases
/// move as the host grows.
pub trait Sealed {}
