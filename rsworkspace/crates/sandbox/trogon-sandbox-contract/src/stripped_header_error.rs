/// A string is not a response header name [`StrippedHeader`](crate::StrippedHeader) accepts.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum StrippedHeaderError {
    #[error("response header name must be a non-empty token of letters, digits, '-', '_' and '.'")]
    Invalid,
}
