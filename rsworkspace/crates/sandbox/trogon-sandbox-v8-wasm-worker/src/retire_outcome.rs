/// What retiring a lease's guest found.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum RetireOutcome {
    /// The guest named by the request is not the one held, or none is.
    Stale,
    /// The guest was held and has now been stopped.
    Purged,
    /// The guest had already finished, or never started.
    Settled,
}
