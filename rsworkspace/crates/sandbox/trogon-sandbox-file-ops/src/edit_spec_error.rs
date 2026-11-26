/// An edit request cannot be carried out as written.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum EditSpecError {
    #[error("edit needs a non-empty string to find")]
    EmptyFind,
    #[error("expected_occurrences must be at least 1")]
    ZeroExpectedOccurrences,
}
