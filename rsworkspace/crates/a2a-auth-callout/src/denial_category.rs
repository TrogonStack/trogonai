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
mod tests {
    use super::*;

    #[test]
    fn each_auth_callout_error_variant_maps_to_category() {
        assert_eq!(
            DenialCategory::from_auth_callout_error(&AuthCalloutError::Connect("x".into())),
            DenialCategory::ServiceUnavailable
        );
        assert_eq!(
            DenialCategory::from_auth_callout_error(&AuthCalloutError::Subscribe(async_nats::SubscribeError::new(
                async_nats::SubscribeErrorKind::InvalidSubject,
            ))),
            DenialCategory::InternalError
        );
        assert_eq!(
            DenialCategory::from_auth_callout_error(&AuthCalloutError::Deserialize(
                serde_json::from_str::<String>("x").unwrap_err()
            )),
            DenialCategory::InvalidRequest
        );
        assert_eq!(
            DenialCategory::from_auth_callout_error(&AuthCalloutError::Serialize(
                serde_json::from_str::<serde_json::Value>("not json").unwrap_err()
            )),
            DenialCategory::InternalError
        );
        assert_eq!(
            DenialCategory::from_auth_callout_error(&AuthCalloutError::Reply(async_nats::PublishError::new(
                async_nats::client::PublishErrorKind::InvalidSubject,
            ))),
            DenialCategory::InternalError
        );
        assert_eq!(
            DenialCategory::from_auth_callout_error(&AuthCalloutError::Jwt(crate::jwt::JwtError::Encode("x".into()))),
            DenialCategory::InternalError
        );
    }

    #[test]
    fn credential_unknown_account() {
        let err = AuthCalloutError::CredentialVerification(CredentialError::UnknownAccount("evil".into()));
        assert_eq!(
            DenialCategory::from_auth_callout_error(&err),
            DenialCategory::UnknownAccount
        );
    }

    #[test]
    fn credential_verifier_unavailable() {
        let err = AuthCalloutError::CredentialVerification(CredentialError::VerifierUnavailable { scheme: "OIDC" });
        assert_eq!(
            DenialCategory::from_auth_callout_error(&err),
            DenialCategory::VerifierUnavailable
        );
    }

    #[test]
    fn credential_invalid_credentials_keeps_category_opaque() {
        let err = AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(
            "OIDC token validation failed: signature invalid for kid abc".into(),
        ));
        let category = DenialCategory::from_auth_callout_error(&err);
        assert_eq!(category, DenialCategory::InvalidCredentials);
        assert!(!category.as_str().contains("signature"));
        assert!(!category.as_str().contains("kid"));
    }

    #[test]
    fn as_str_covers_every_variant() {
        assert_eq!(DenialCategory::InvalidCredentials.as_str(), "invalid_credentials");
        assert_eq!(DenialCategory::UnknownAccount.as_str(), "unknown_account");
        assert_eq!(DenialCategory::InvalidRequest.as_str(), "invalid_request");
        assert_eq!(DenialCategory::VerifierUnavailable.as_str(), "verifier_unavailable");
        assert_eq!(DenialCategory::InternalError.as_str(), "internal_error");
        assert_eq!(DenialCategory::ServiceUnavailable.as_str(), "service_unavailable");
    }

    #[test]
    fn wire_format_and_internal_map_to_internal_error() {
        assert_eq!(
            DenialCategory::from_auth_callout_error(&AuthCalloutError::WireFormat("x".into())),
            DenialCategory::InternalError
        );
        assert_eq!(
            DenialCategory::from_auth_callout_error(&AuthCalloutError::Internal("x".into())),
            DenialCategory::InternalError
        );
    }

    #[test]
    fn credential_invalid_request_maps_to_invalid_request() {
        let err =
            AuthCalloutError::CredentialVerification(CredentialError::InvalidRequest("request missing account".into()));
        assert_eq!(
            DenialCategory::from_auth_callout_error(&err),
            DenialCategory::InvalidRequest
        );
    }
}
