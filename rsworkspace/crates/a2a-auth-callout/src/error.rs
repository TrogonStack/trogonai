use std::fmt;
use std::path::PathBuf;

use crate::jwt::JwtError;

#[derive(Debug)]
pub enum AuthCalloutError {
    Connect(String),
    Subscribe(String),
    Deserialize(serde_json::Error),
    Serialize(serde_json::Error),
    Reply(String),
    CredentialVerification(String),
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
            Self::Subscribe(msg) => write!(f, "subscribe to auth callout subject failed: {msg}"),
            Self::Deserialize(_) => f.write_str("failed to deserialize auth callout request"),
            Self::Serialize(_) => f.write_str("failed to serialize auth callout response"),
            Self::Reply(msg) => write!(f, "failed to publish auth callout reply: {msg}"),
            Self::CredentialVerification(msg) => write!(f, "credential verification failed: {msg}"),
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
        assert!(
            AuthCalloutError::Subscribe("denied".into())
                .to_string()
                .contains("auth callout subject")
        );
    }

    #[test]
    fn display_credential_verification() {
        assert!(
            AuthCalloutError::CredentialVerification("bad token".into())
                .to_string()
                .contains("credential verification")
        );
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
        assert!(
            AuthCalloutError::Reply("x".into())
                .to_string()
                .contains("publish auth callout reply")
        );
        assert!(
            AuthCalloutError::WireFormat("x".into())
                .to_string()
                .contains("wire format")
        );
        assert!(AuthCalloutError::Internal("x".into()).to_string().contains("internal"));
    }
}
