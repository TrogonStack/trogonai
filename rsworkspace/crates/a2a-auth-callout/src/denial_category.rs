use crate::error::AuthCalloutError;

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
            AuthCalloutError::CredentialVerification(msg) => Self::from_credential_message(msg),
        }
    }

    fn from_credential_message(msg: &str) -> Self {
        if msg.contains("not allowlisted") {
            return Self::UnknownAccount;
        }
        if msg.contains("not configured") {
            return Self::VerifierUnavailable;
        }
        if msg.contains("request missing account") || msg.contains("no credential material") {
            return Self::InvalidRequest;
        }
        if msg.contains("scheme but") && msg.contains("missing") {
            return Self::InvalidRequest;
        }
        Self::InvalidCredentials
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
            DenialCategory::from_auth_callout_error(&AuthCalloutError::Subscribe("x".into())),
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
            DenialCategory::from_auth_callout_error(&AuthCalloutError::Reply("x".into())),
            DenialCategory::InternalError
        );
        assert_eq!(
            DenialCategory::from_auth_callout_error(&AuthCalloutError::Jwt(crate::jwt::JwtError::Encode("x".into()))),
            DenialCategory::InternalError
        );
    }

    #[test]
    fn credential_unknown_account() {
        let err = AuthCalloutError::CredentialVerification("requested account \"evil\" not allowlisted".into());
        assert_eq!(
            DenialCategory::from_auth_callout_error(&err),
            DenialCategory::UnknownAccount
        );
    }

    #[test]
    fn credential_verifier_unavailable() {
        let err = AuthCalloutError::CredentialVerification("OIDC verifier not configured".into());
        assert_eq!(
            DenialCategory::from_auth_callout_error(&err),
            DenialCategory::VerifierUnavailable
        );
    }

    #[test]
    fn credential_oidc_failure_maps_to_invalid_credentials_not_internal_detail() {
        let err = AuthCalloutError::CredentialVerification(
            "OIDC token validation failed: signature invalid for kid abc".into(),
        );
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
    fn credential_message_request_missing_account_is_invalid_request() {
        let err = AuthCalloutError::CredentialVerification("request missing account".into());
        assert_eq!(
            DenialCategory::from_auth_callout_error(&err),
            DenialCategory::InvalidRequest
        );
        let err = AuthCalloutError::CredentialVerification("no credential material in request".into());
        assert_eq!(
            DenialCategory::from_auth_callout_error(&err),
            DenialCategory::InvalidRequest
        );
    }

    #[test]
    fn credential_message_scheme_missing_is_invalid_request() {
        let err = AuthCalloutError::CredentialVerification("api_key scheme but apikey is missing".into());
        assert_eq!(
            DenialCategory::from_auth_callout_error(&err),
            DenialCategory::InvalidRequest
        );
    }
}
