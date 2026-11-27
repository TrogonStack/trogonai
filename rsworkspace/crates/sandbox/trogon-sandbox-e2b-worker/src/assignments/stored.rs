use super::assignment::Assignment;

/// An assignment as read, with the revision a compare-and-swap update must name.
pub(super) struct Stored {
    pub assignment: Assignment,
    pub revision: u64,
}
