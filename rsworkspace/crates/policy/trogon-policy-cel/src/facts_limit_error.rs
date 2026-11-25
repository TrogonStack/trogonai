/// Why a set of facts is not evaluated.
///
/// Facts are bounded before any rule reads them, because the static cost of a
/// rule assumes them to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FactsLimitError {
    /// The facts hold more bytes than the limit.
    #[error("facts exceed the input limit of {limit} bytes")]
    TooManyBytes {
        /// The most bytes allowed.
        limit: usize,
    },
    /// A list or map holds more entries than the limit.
    #[error("a fact holds {actual} entries, the limit is {limit}")]
    CollectionTooLong {
        /// The length found.
        actual: usize,
        /// The longest allowed.
        limit: usize,
    },
    /// Lists and maps nest deeper than the limit.
    #[error("facts nest deeper than {limit} levels")]
    NestedTooDeep {
        /// The deepest nesting allowed.
        limit: usize,
    },
}
