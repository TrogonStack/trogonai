/// Whether a manifest write created the record or found one already there.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum RecordOutcome {
    Created,
    AlreadyExists,
}
