/// A string is not standard base64 [`Base64Data`](crate::Base64Data) accepts.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[error("value must be standard base64 with padding")]
pub struct Base64DataError {
    #[source]
    pub(crate) source: base64::DecodeError,
}
