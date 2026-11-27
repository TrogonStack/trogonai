/// A string is not a secret name [`SecretName`](crate::SecretName) accepts.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SecretNameError {
    #[error("secret name must not be empty")]
    Empty,
    #[error("secret name must contain only ASCII letters, digits, '.', '_', or '-'")]
    InvalidCharacter,
}
