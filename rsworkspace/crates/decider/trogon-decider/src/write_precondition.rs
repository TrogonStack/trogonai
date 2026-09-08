/// Concurrency guard a command's meaning requires when its events are appended.
///
/// Declared per command via [`Decider::WRITE_PRECONDITION`](crate::Decider::WRITE_PRECONDITION).
/// Every variant is a predicate over the stream's state at append time; the persistence layer
/// rejects the append when the predicate does not hold.
///
/// The const answers *"what does this command's meaning require?"*, which is a compile-time
/// property of the command. It is distinct from a client's expected revision, which arrives per
/// request and is supplied separately by the caller. A caller's revision can only strengthen the
/// declared guard, never weaken it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritePrecondition {
    /// Append only if the stream is still exactly as replay observed it.
    ///
    /// The optimistic-concurrency default for an ordinary state transition: it rejects a decision
    /// computed from state that another writer has since invalidated. The runtime supplies the
    /// observed position, which is why this variant names the intent rather than a revision.
    StreamUnchanged,
    /// Append only if the stream is empty, so the first of several concurrent writers wins.
    ///
    /// The guard for a creation command. Jointly unsatisfiable with a caller-supplied expected
    /// revision, which asserts the stream already reached that revision.
    NoStream,
    /// Append only if the stream already contains events.
    ///
    /// For a decider whose [`initial_state`](crate::Decider::initial_state) is itself a valid
    /// state, folding zero events and folding a created-but-quiet aggregate produce the same
    /// state, so [`decide`](crate::Decider::decide) cannot tell "never created" from "created,
    /// nothing has happened yet". That distinction lives only in the stream's emptiness, which
    /// `decide` never observes.
    StreamExists,
    /// Append regardless of the stream's current state.
    ///
    /// Appropriate only when the command's events commute, so ordering against concurrent writers
    /// carries no meaning and guarding is pure cost.
    Any,
}
