use super::constants::MAX_DNS1123_LABEL;

/// Why a string is not a DNS-1123 label.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub(super) enum DnsLabelError {
    #[error("empty")]
    Empty,
    #[error("{length} characters, over the {MAX_DNS1123_LABEL}-character limit")]
    TooLong { length: usize },
    #[error("contains {character:?}, expected lowercase alphanumerics and '-'")]
    InvalidCharacter { character: char },
    #[error("must start with a lowercase alphanumeric")]
    BadStart,
    #[error("must end with a lowercase alphanumeric")]
    BadEnd,
}
