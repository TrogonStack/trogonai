use std::time::Duration;

/// How often the reaper sweeps. Long, because an actor is only reaped after it has been
/// unclaimed across a sweep, and the cost of holding a leaked actor for a few minutes is
/// far below the cost of racing a lease that is still writing its index entry.
pub const REAP_INTERVAL: Duration = Duration::from_secs(300);
