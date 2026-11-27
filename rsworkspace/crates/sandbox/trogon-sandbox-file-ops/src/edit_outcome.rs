/// What an exact-string edit decided.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum EditOutcome {
    /// The count matched; `content` is the file with every occurrence replaced.
    Replaced { content: Vec<u8>, occurrences: u32 },
    /// The file holds a different number of occurrences than the edit expected.
    Rejected { occurrences: u32 },
}
