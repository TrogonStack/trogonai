use crate::{SecretName, SecretTarget};

/// A list of bindings is not a collection [`SecretBindings`](crate::SecretBindings) accepts.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum SecretBindingsError {
    #[error("secret bindings support at most {max} entries, got {got}")]
    TooMany { max: usize, got: usize },
    #[error("duplicate secret binding name '{0}'")]
    DuplicateName(SecretName),
    #[error("duplicate secret binding target {0}")]
    DuplicateTarget(SecretTarget),
}
