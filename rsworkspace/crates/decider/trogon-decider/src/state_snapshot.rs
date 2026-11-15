/// Associates a decider state with the payload persisted in a snapshot.
///
/// Uses [`From`] to restore state and [`AsRef`] to borrow its persisted payload.
pub trait StateSnapshot: From<Self::Payload> + AsRef<Self::Payload> {
    /// Value encoded in and decoded from a snapshot.
    type Payload;
}
