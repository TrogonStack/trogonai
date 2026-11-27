/// A stored credential cannot satisfy the scope a request asked for.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum CredentialMaterialError {
    #[error("credential material does not match requested scope")]
    ScopeMismatch,
}
