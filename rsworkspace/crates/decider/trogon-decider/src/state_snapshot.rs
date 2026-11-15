/// Maps a decider's state to the value persisted in a snapshot.
///
/// The state may carry a local type while the snapshot retains an existing wire format.
pub trait StateSnapshot: Sized {
    /// Value encoded in and decoded from a snapshot.
    type Payload;

    /// Restores state from a decoded snapshot payload.
    fn from_snapshot_payload(payload: Self::Payload) -> Self;

    /// Returns the value to encode in a snapshot.
    fn snapshot_payload(&self) -> &Self::Payload;
}
