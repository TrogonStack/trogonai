use std::time::Duration;

/// Seconds between sweeps of the idle warm containers.
pub(super) const WARM_POOL_SWEEP_INTERVAL: Duration = Duration::from_secs(60);
