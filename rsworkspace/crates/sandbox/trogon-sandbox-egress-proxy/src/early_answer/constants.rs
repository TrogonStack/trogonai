use std::time::Duration;

/// How long a body that is drained after its answer may go without a byte
/// before the tunnel stops waiting for the rest.
pub(crate) const DRAIN_QUIET: Duration = Duration::from_secs(1);
