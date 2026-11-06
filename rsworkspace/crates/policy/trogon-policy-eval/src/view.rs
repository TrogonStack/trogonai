use std::sync::Arc;

use crate::snapshot::PolicySnapshot;

/// Where an enforcement point gets the snapshot it evaluates against.
///
/// Deliberately synchronous (ADR#0026): a view hands out what it already holds
/// and never waits on a remote source. A projection that refreshes in the
/// background swaps the snapshot it hands out and stops advancing `as_of` when
/// it falls behind, which makes the evaluator refuse.
pub trait PolicyView: Send + Sync {
    /// The current snapshot.
    fn snapshot(&self) -> Arc<PolicySnapshot>;
}

impl<V> PolicyView for Arc<V>
where
    V: PolicyView + ?Sized,
{
    fn snapshot(&self) -> Arc<PolicySnapshot> {
        self.as_ref().snapshot()
    }
}
