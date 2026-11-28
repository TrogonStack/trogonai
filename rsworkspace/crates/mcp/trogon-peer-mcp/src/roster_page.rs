/// Whether a roster read covers every peer or only part of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RosterPage {
    /// The first page, with no page after it.
    Whole,
    Partial,
}
