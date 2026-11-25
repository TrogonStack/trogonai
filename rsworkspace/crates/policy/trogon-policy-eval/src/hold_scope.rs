/// How far a hold reaches (ADR#0071 Decision 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HoldScope {
    /// Refuses new Sessions of the subject and new delegations into it.
    /// Running Sessions finish on their own terms.
    NewWork,
    /// Refuses the same, and fails every running Session closed at its next
    /// protected action. Nothing is suspended.
    AllProtectedActions,
}
