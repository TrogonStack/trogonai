use std::path::PathBuf;

use crate::jwt::JwtError;

/// Typed reason a credential verifier rejected a caller. Replaces the
/// previous `String` payload on `AuthCalloutError::CredentialVerification`
/// so the denial category is derivable from the variant tag, not from
/// substring-matching the error message.
#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    /// The caller requested an account that isn't in the allowlist (or
    /// otherwise doesn't resolve).
    #[error("requested account {0:?} not allowlisted")]
    UnknownAccount(String),
    /// The configured verifier for a scheme is missing or not initialized.
    #[error("{scheme} verifier not configured")]
    VerifierUnavailable { scheme: &'static str },
    /// The request shape was wrong before the verifier could even try
    /// (missing required fields, empty material, scheme/material mismatch).
    #[error("credential request invalid: {0}")]
    InvalidRequest(String),
    /// The verifier ran and refused the credential material itself.
    #[error("credential verification failed: {0}")]
    InvalidCredentials(String),
}

impl From<CredentialError> for AuthCalloutError {
    fn from(e: CredentialError) -> Self {
        AuthCalloutError::CredentialVerification(e)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthCalloutError {
    #[error("NATS connect failed: {0}")]
    Connect(String),
    /// Subscribing to `$SYS.REQ.USER.AUTH` failed — wraps the typed
    /// async_nats subscribe error so the source chain isn't lost.
    #[error("subscribe to auth callout subject failed")]
    Subscribe(#[source] async_nats::SubscribeError),
    #[error("failed to deserialize auth callout request")]
    Deserialize(#[source] serde_json::Error),
    #[error("failed to serialize auth callout response")]
    Serialize(#[source] serde_json::Error),
    /// Publishing a reply on the auth-callout inbox failed — wraps the
    /// typed async_nats publish error.
    #[error("failed to publish auth callout reply")]
    Reply(#[source] async_nats::PublishError),
    #[error("{0}")]
    CredentialVerification(#[source] CredentialError),
    #[error("JWT operation failed")]
    Jwt(#[source] JwtError),
    #[error("auth callout wire format error: {0}")]
    WireFormat(String),
    #[error("internal error: {0}")]
    Internal(String),
    /// Required process-edge environment variable was missing.
    #[error("required environment variable {0} is not set")]
    MissingEnvVar(&'static str),
    /// `AUTH_CALLOUT_SIGNING_KEY_SOURCE` named a backend that doesn't exist.
    #[error("unknown AUTH_CALLOUT_SIGNING_KEY_SOURCE: {0} (expected env, file, or vault)")]
    UnknownSigningKeySource(String),
    /// `vault` custody isn't wired yet; the loader rejects it explicitly.
    #[error("AUTH_CALLOUT_SIGNING_KEY_SOURCE=vault is not wired yet")]
    VaultNotConfigured,
    /// Reading a signing-key file from disk failed.
    #[error("failed to read signing key at {}", .path.display())]
    KeyLoadIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// A signing-key file's bytes weren't valid UTF-8.
    #[error("signing key file must be UTF-8 NKey seed")]
    KeyLoadUtf8(#[source] std::str::Utf8Error),
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    #[test]
    fn display_connect() {
        assert!(
            AuthCalloutError::Connect("refused".into())
                .to_string()
                .contains("NATS connect failed")
        );
    }

    #[test]
    fn display_subscribe() {
        use std::error::Error;
        let e = AuthCalloutError::Subscribe(async_nats::SubscribeError::new(
            async_nats::SubscribeErrorKind::InvalidSubject,
        ));
        assert!(e.to_string().contains("auth callout subject"));
        assert!(Error::source(&e).is_some());
    }

    #[test]
    fn display_credential_verification() {
        let e = AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials("bad token".into()));
        assert!(e.to_string().contains("credential verification"));
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn display_jwt_variant() {
        let e = AuthCalloutError::Jwt(crate::jwt::JwtError::Encode("key missing".into()));
        assert_eq!(e.to_string(), "JWT operation failed");
        assert!(Error::source(&e).is_some());
    }

    #[test]
    fn display_missing_env_var_unknown_source_and_vault() {
        assert!(
            AuthCalloutError::MissingEnvVar("X")
                .to_string()
                .contains("X is not set")
        );
        assert!(
            AuthCalloutError::UnknownSigningKeySource("foo".into())
                .to_string()
                .contains("foo")
        );
        assert!(
            AuthCalloutError::VaultNotConfigured
                .to_string()
                .contains("not wired yet")
        );
    }

    #[test]
    fn display_and_source_for_key_load_variants() {
        let io = AuthCalloutError::KeyLoadIo {
            path: std::path::PathBuf::from("/tmp/x"),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "nope"),
        };
        assert!(io.to_string().contains("/tmp/x"));
        assert!(Error::source(&io).is_some());

        let bad: Vec<u8> = vec![0xff, 0xfe];
        let utf8 = AuthCalloutError::KeyLoadUtf8(std::str::from_utf8(&bad).unwrap_err());
        assert!(utf8.to_string().contains("UTF-8"));
        assert!(Error::source(&utf8).is_some());
    }

    #[test]
    fn source_for_deserialize() {
        let e = AuthCalloutError::Deserialize(serde_json::from_str::<String>("x").unwrap_err());
        assert!(e.source().is_some());
    }

    #[test]
    fn source_for_connect_is_none() {
        assert!(AuthCalloutError::Connect("x".into()).source().is_none());
    }

    #[test]
    fn display_covers_remaining_variants() {
        let de = serde_json::from_str::<String>("x").unwrap_err();
        assert!(
            AuthCalloutError::Deserialize(de)
                .to_string()
                .contains("deserialize auth callout request")
        );
        let se = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        assert!(
            AuthCalloutError::Serialize(se)
                .to_string()
                .contains("serialize auth callout response")
        );
        let reply_err = AuthCalloutError::Reply(async_nats::PublishError::new(
            async_nats::client::PublishErrorKind::InvalidSubject,
        ));
        assert!(reply_err.to_string().contains("publish auth callout reply"));
        assert!(
            AuthCalloutError::WireFormat("x".into())
                .to_string()
                .contains("wire format")
        );
        assert!(AuthCalloutError::Internal("x".into()).to_string().contains("internal"));
    }
}
