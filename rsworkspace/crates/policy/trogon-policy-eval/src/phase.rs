/// Where in a Session's life an action is being evaluated.
///
/// Holds read it: a `NEW_WORK` hold refuses what starts new work and lets
/// running Sessions finish, while `ALL_PROTECTED_ACTIONS` also stops a running
/// Session at its next protected action (ADR#0071 Decision 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Phase {
    /// A Session is being admitted.
    SessionAdmission,
    /// A running Session is about to take a protected action: a model
    /// request, a tool call, or a delegation dispatch.
    ProtectedAction,
}
