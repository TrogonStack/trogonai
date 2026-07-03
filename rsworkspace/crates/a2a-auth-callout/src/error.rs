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
mod tests;
