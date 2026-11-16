/// One step of starting to read invalidations, for a scripted stand-in to
/// refuse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InvalidationStep {
    Stream,
    Consumer,
    Messages,
}
