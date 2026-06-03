//! SDK error type. Bundles transport, signing, and protocol errors so callers
//! can match on a single enum.

use trogon_identity_types::aauth::Requirement;

#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    #[error("HTTP transport: {0}")]
    Http(#[from] reqwest::Error),
    #[error("NATS transport: {0}")]
    Nats(String),
    #[error("encode JWT: {0}")]
    Jwt(#[from] jsonwebtoken::errors::Error),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    #[error("server returned {status}: {body}")]
    Server { status: u16, body: String },
    #[error("interaction required at {url}")]
    Interaction { url: String, code: Option<String> },
    #[error("agent token missing or expired")]
    AgentTokenUnavailable,
    #[error("requirement {0:?} not satisfiable by SDK")]
    UnsupportedRequirement(Requirement),
}
