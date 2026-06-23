use crate::error::{AuthCalloutError, CredentialError};

/// Opaque denial category returned on the wire in `nats.error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenialCategory {
    InvalidCredentials,
    UnknownAccount,
    InvalidRequest,
    VerifierUnavailable,
    InternalError,
    ServiceUnavailable,
}

impl DenialCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidCredentials => "invalid_credentials",
            Self::UnknownAccount => "unknown_account",
            Self::InvalidRequest => "invalid_request",
            Self::VerifierUnavailable => "verifier_unavailable",
            Self::InternalError => "internal_error",
            Self::ServiceUnavailable => "service_unavailable",
        }
    }

    pub fn from_auth_callout_error(error: &AuthCalloutError) -> Self {
        match error {
            AuthCalloutError::Connect(_) => Self::ServiceUnavailable,
            AuthCalloutError::Subscribe(_) => Self::InternalError,
            AuthCalloutError::Deserialize(_) => Self::InvalidRequest,
            AuthCalloutError::Serialize(_) => Self::InternalError,
            AuthCalloutError::Reply(_) => Self::InternalError,
            AuthCalloutError::Jwt(_) => Self::InternalError,
            AuthCalloutError::WireFormat(_) => Self::InternalError,
            AuthCalloutError::Internal(_) => Self::InternalError,
            AuthCalloutError::MissingEnvVar(_)
            | AuthCalloutError::UnknownSigningKeySource(_)
            | AuthCalloutError::VaultNotConfigured
            | AuthCalloutError::KeyLoadIo { .. }
            | AuthCalloutError::KeyLoadUtf8(_) => Self::InternalError,
            AuthCalloutError::CredentialVerification(e) => Self::from_credential_error(e),
        }
    }

    fn from_credential_error(e: &CredentialError) -> Self {
        match e {
            CredentialError::UnknownAccount(_) => Self::UnknownAccount,
            CredentialError::VerifierUnavailable { .. } => Self::VerifierUnavailable,
            CredentialError::InvalidRequest(_) => Self::InvalidRequest,
            CredentialError::InvalidCredentials(_) => Self::InvalidCredentials,
        }
    }
}

#[cfg(test)]
mod tests;
