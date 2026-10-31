use std::error::Error;

use trogon_aauth_verify::{HttpPopError, NonceChallenge};

use crate::edge_identity::{BearerSubjectError, CredentialSubject, CredentialSubjectError};

#[allow(dead_code)]
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("missing Authorization header")]
    MissingAuthorization,
    #[error(transparent)]
    EdgeAuth(#[from] EdgeAuthError),
    #[error("request body was not UTF-8: {0}")]
    Utf8Body(#[source] std::str::Utf8Error),
    #[error("failed to deserialize JSON-RPC body: {0}")]
    Deserialize(#[source] serde_json::Error),
    #[error("failed to serialize bridge response: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("missing X-A2A-Agent-Id header")]
    MissingAgentHeader,
    #[error("JSON-RPC body missing method")]
    MissingJsonRpcMethod,
    #[error("JSON-RPC method is not an A2A method")]
    UnknownJsonRpcMethod,
    #[error("auth callout mint failed: {0}")]
    Mint(String),
    #[error("minted user JWT names no caller principal: {0}")]
    MintedPrincipal(#[source] a2a_auth_callout::jwt::JwtError),
    #[error("auth callout mint failed: {0}")]
    MintCredential(#[source] a2a_auth_callout::BridgeCredentialError),
    #[error("NATS gateway publish failed: {0}")]
    NatsPublish(String),
    #[error("JetStream SSE consumer attach failed: {0}")]
    JetStreamConsume(String),
    #[error("HTTPS upstream forward failed: {0}")]
    UpstreamHttps(String),
    #[error("JSON-RPC streaming request missing usable id")]
    MissingJsonRpcId,
    #[error("invalid streaming RPC params: {0}")]
    StreamingParams(String),
    #[error("gateway unary returned JSON-RPC error: {0}")]
    JsonRpcUpstream(String),
    #[error("catalog registration publish failed: {0}")]
    CatalogRegistration(String),
    #[error("invalid agent identifier: {0}")]
    InvalidAgent(String),
    #[error("failed to build HTTP response: {0}")]
    ResponseBuild(String),
}

/// Why the edge refused a request before asking the auth callout to mint for
/// it. Each displays only what a caller may learn; [`EdgeAuthError::detail`]
/// carries the cause for the edge's own logs.
#[derive(Debug, thiserror::Error)]
pub enum EdgeAuthError {
    #[error("request carries no AAuth proof of possession")]
    Unsigned(#[source] HttpPopError),
    #[error("AAuth proof of possession did not verify")]
    Unverified(#[source] HttpPopError),
    #[error("request has to bind a server nonce issued for it")]
    NonceRequired(NonceChallenge),
    #[error("AAuth proof of possession could not be checked")]
    Unavailable(#[source] HttpPopError),
    #[error("bearer credential does not name the caller that signed the request")]
    BearerMismatch {
        verified: CredentialSubject,
        bearer: CredentialSubject,
    },
    #[error("bearer credential names no caller")]
    UnidentifiedBearer(#[source] BearerSubjectError),
    #[error("AAuth token names no caller")]
    UnidentifiedPresenter(#[source] CredentialSubjectError),
}

impl EdgeAuthError {
    /// Sorts a verifier failure by what the caller has to do about it: sign at
    /// all, sign correctly, bind the nonce it was handed, or retry once the
    /// replay store answers again.
    #[must_use]
    pub fn from_pop(error: HttpPopError) -> Self {
        match error {
            HttpPopError::MissingHeader(_) => Self::Unsigned(error),
            HttpPopError::NonceRequired(challenge) => Self::NonceRequired(challenge),
            HttpPopError::Backend(_) | HttpPopError::NonceIssue(_) => Self::Unavailable(error),
            other => Self::Unverified(other),
        }
    }

    /// Whether the request is anonymous to the edge: it carries no AAuth
    /// proof of possession, or its bearer credential names no caller. Shadow
    /// mode still refuses these, since it only relaxes a proof that fails,
    /// never one that is absent.
    #[must_use]
    pub fn leaves_request_anonymous(&self) -> bool {
        matches!(self, Self::Unsigned(_) | Self::UnidentifiedBearer(_))
    }

    /// The refusal with every cause behind it, for logs rather than callers.
    #[must_use]
    pub fn detail(&self) -> String {
        let mut detail = match self {
            Self::BearerMismatch { verified, bearer } => format!("{self}: verified {verified}, bearer {bearer}"),
            Self::NonceRequired(challenge) => match challenge.refused() {
                Some(refused) => format!("{self}: {refused}"),
                None => self.to_string(),
            },
            _ => self.to_string(),
        };
        let mut cause = self.source();
        while let Some(error) = cause {
            let text = error.to_string();
            if !detail.contains(&text) {
                detail.push_str(": ");
                detail.push_str(&text);
            }
            cause = error.source();
        }
        detail
    }
}

#[cfg(test)]
mod tests;
