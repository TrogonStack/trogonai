/// What to do with an unsettled freeze marker.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FreezeResolution {
    /// Drop the marker and accept the worker's state as it is. The recorded
    /// status stays what clients were told, so a worker that disagrees stays
    /// out of step until the lease is next paused, resumed or ended.
    Clear,
    /// Reset the attempt count so the reconciler asks the worker again, with
    /// the usual backoff, until it settles or gives up once more.
    Retry,
}
