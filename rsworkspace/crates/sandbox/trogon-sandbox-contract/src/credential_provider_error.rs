/// A string does not name a [`CredentialProvider`](crate::CredentialProvider).
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum CredentialProviderError {
    #[error("unknown credential provider")]
    Unknown,
}
