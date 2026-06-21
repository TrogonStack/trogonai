use std::fmt;
use std::path::PathBuf;

use crate::jwt::JwtError;

/// Typed reason a credential verifier rejected a caller. Replaces the
/// previous `String` payload on `AuthCalloutError::CredentialVerification`
/// so the denial category is derivable from the variant tag, not from
/// substring-matching the error message.
#[derive(Debug)]
pub enum CredentialError {
    /// The caller requested an account that isn't in the allowlist (or
    /// otherwise doesn't resolve).
    UnknownAccount(String),
    /// The configured verifier for a scheme is missing or not initialized.
    VerifierUnavailable { scheme: &'static str },
    /// The request shape was wrong before the verifier could even try
    /// (missing required fields, empty material, scheme/material mismatch).
    InvalidRequest(String),
    /// The verifier ran and refused the credential material itself.
    InvalidCredentials(String),
}

impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownAccount(name) => write!(f, "requested account {name:?} not allowlisted"),
            Self::VerifierUnavailable { scheme } => write!(f, "{scheme} verifier not configured"),
            Self::InvalidRequest(msg) => write!(f, "credential request invalid: {msg}"),
            Self::InvalidCredentials(msg) => write!(f, "credential verification failed: {msg}"),
        }
    }
}

impl std::error::Error for CredentialError {}

impl From<CredentialError> for AuthCalloutError {
    fn from(e: CredentialError) -> Self {
        AuthCalloutError::CredentialVerification(e)
    }
}

#[derive(Debug)]
pub enum AuthCalloutError {
    Connect(String),
    /// Subscribing to `$SYS.REQ.USER.AUTH` failed — wraps the typed
    /// async_nats subscribe error so the source chain isn't lost.
    Subscribe(async_nats::SubscribeError),
    Deserialize(serde_json::Error),
    Serialize(serde_json::Error),
    /// Publishing a reply on the auth-callout inbox failed — wraps the
    /// typed async_nats publish error.
    Reply(async_nats::PublishError),
    CredentialVerification(CredentialError),
    Jwt(JwtError),
    WireFormat(String),
    Internal(String),
    /// Required process-edge environment variable was missing.
    MissingEnvVar(&'static str),
    /// `AUTH_CALLOUT_SIGNING_KEY_SOURCE` named a backend that doesn't exist.
    UnknownSigningKeySource(String),
    /// `vault` custody isn't wired yet; the loader rejects it explicitly.
    VaultNotConfigured,
    /// Reading a signing-key file from disk failed.
    KeyLoadIo {
        path: PathBuf,
        source: std::io::Error,
    },
    /// A signing-key file's bytes weren't valid UTF-8.
    KeyLoadUtf8(std::str::Utf8Error),
}

impl fmt::Display for AuthCalloutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(msg) => write!(f, "NATS connect failed: {msg}"),
            Self::Subscribe(_) => f.write_str("subscribe to auth callout subject failed"),
            Self::Deserialize(_) => f.write_str("failed to deserialize auth callout request"),
            Self::Serialize(_) => f.write_str("failed to serialize auth callout response"),
            Self::Reply(_) => f.write_str("failed to publish auth callout reply"),
            Self::CredentialVerification(e) => write!(f, "{e}"),
            Self::Jwt(_) => f.write_str("JWT operation failed"),
            Self::WireFormat(msg) => write!(f, "auth callout wire format error: {msg}"),
            Self::Internal(msg) => write!(f, "internal error: {msg}"),
            Self::MissingEnvVar(name) => write!(f, "required environment variable {name} is not set"),
            Self::UnknownSigningKeySource(kind) => {
                write!(
                    f,
                    "unknown AUTH_CALLOUT_SIGNING_KEY_SOURCE: {kind} (expected env, file, or vault)"
                )
            }
            Self::VaultNotConfigured => f.write_str("AUTH_CALLOUT_SIGNING_KEY_SOURCE=vault is not wired yet"),
            Self::KeyLoadIo { path, .. } => write!(f, "failed to read signing key at {}", path.display()),
            Self::KeyLoadUtf8(_) => f.write_str("signing key file must be UTF-8 NKey seed"),
        }
    }
}

impl std::error::Error for AuthCalloutError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Deserialize(e) | Self::Serialize(e) => Some(e),
            Self::Subscribe(e) => Some(e),
            Self::Reply(e) => Some(e),
            Self::CredentialVerification(e) => Some(e),
            Self::Jwt(e) => Some(e),
            Self::KeyLoadIo { source, .. } => Some(source),
            Self::KeyLoadUtf8(e) => Some(e),
            _ => None,
        }
    }
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
