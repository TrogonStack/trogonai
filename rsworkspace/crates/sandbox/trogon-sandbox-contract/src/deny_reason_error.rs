/// A string is not a [`DenyReason`](crate::DenyReason).
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum DenyReasonError {
    #[error("a deny reason cannot be empty")]
    Empty,
}
