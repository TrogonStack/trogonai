use trogon_sandbox_contract::SecretName;

use crate::JwtMaskError;

/// Why the captures of a mask plan could not be registered.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum CaptureMaskError {
    #[error("secret binding '{0}' has no entry in the registry")]
    UnknownBinding(SecretName),
    #[error("a value captured for '{0}' holds a control character, which cannot be substituted into HTTP safely")]
    ControlCharacter(SecretName),
    #[error("the name for capture {index} of '{binding}' is not a valid secret name")]
    CaptureEntryName { binding: SecretName, index: usize },
    #[error("a captured JWT of '{binding}' cannot be masked: {source}")]
    Jwt { binding: SecretName, source: JwtMaskError },
}
