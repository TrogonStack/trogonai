use std::num::NonZeroU64;

/// How often a command's execution should persist a snapshot of the decider's state.
///
/// Declared per command via [`Decider::SNAPSHOT_CADENCE`](crate::Decider::SNAPSHOT_CADENCE) so the
/// native and sandboxed execution paths read the same declaration: the WASM host receives it in the
/// module descriptor rather than choosing its own, which is what keeps the two paths comparable.
///
/// Cadence is a cost trade, not a correctness one. Replaying more events is always sound, just
/// slower, which is why the const defaults to [`SnapshotCadence::Never`] where
/// [`WritePrecondition`](crate::WritePrecondition) has no default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotCadence {
    /// Never snapshot. Every execution replays the stream from its beginning.
    Never,
    /// Snapshot once at least this many events have been read or appended since the last snapshot.
    EveryEvents(NonZeroU64),
}

impl SnapshotCadence {
    /// Returns the cadence for snapshotting every `events` events, or [`SnapshotCadence::Never`]
    /// when `events` is zero.
    ///
    /// A cadence arriving from a WASM module descriptor crosses a `u64` field that cannot express
    /// non-zero, so a host needs this to admit a declared frequency without panicking on a value
    /// the WIT type system let through.
    pub const fn every_events(events: u64) -> Self {
        match NonZeroU64::new(events) {
            Some(events) => Self::EveryEvents(events),
            None => Self::Never,
        }
    }

    /// Returns the declared frequency, or `None` when this cadence never snapshots.
    pub const fn frequency(self) -> Option<NonZeroU64> {
        match self {
            Self::Never => None,
            Self::EveryEvents(frequency) => Some(frequency),
        }
    }

    /// Returns whether `events_since_snapshot` events are enough to make a snapshot due.
    pub const fn is_due(self, events_since_snapshot: u64) -> bool {
        match self {
            Self::Never => false,
            Self::EveryEvents(frequency) => events_since_snapshot >= frequency.get(),
        }
    }
}

#[cfg(test)]
mod tests;
