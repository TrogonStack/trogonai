/// Outcome of a label patch fenced on the object's `resourceVersion`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FencedPatchOutcome {
    /// The patch applied: this caller observed the object's latest state.
    Applied,
    /// `409 Conflict`: the object changed since the read that authorized this
    /// write. Not an error and not permission to retry blindly: re-observe.
    PreconditionFailed,
}
