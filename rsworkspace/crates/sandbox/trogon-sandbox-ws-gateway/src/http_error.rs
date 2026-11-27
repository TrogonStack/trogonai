use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use trogon_sandbox_auth::{ApiKeyStoreError, BearerTokenError};

use crate::deny::deny;
use crate::{EntropyError, SessionStoreError};

/// Why a session or socket request was refused.
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("the API key header is missing")]
    MissingApiKey,
    #[error("the API key is not valid")]
    InvalidApiKey,
    #[error("API key lookup failed: {0}")]
    ApiKeyBackend(#[from] ApiKeyStoreError),
    #[error("the session token is missing")]
    MissingToken,
    #[error("the session token is not valid: {0}")]
    InvalidToken(#[from] BearerTokenError),
    #[error("the session does not exist or has expired")]
    InvalidSession,
    #[error("session store failed: {0}")]
    SessionStore(#[from] SessionStoreError),
    #[error("a session token could not be minted: {0}")]
    Entropy(#[from] EntropyError),
}

impl HttpError {
    fn status_and_code(&self) -> (StatusCode, &'static str) {
        match self {
            Self::MissingApiKey => (StatusCode::UNAUTHORIZED, "missing_api_key"),
            Self::InvalidApiKey => (StatusCode::UNAUTHORIZED, "invalid_api_key"),
            Self::ApiKeyBackend(_) => (StatusCode::INTERNAL_SERVER_ERROR, "api_key_backend_failure"),
            Self::MissingToken | Self::InvalidToken(_) => (StatusCode::UNAUTHORIZED, "missing_or_invalid_token"),
            Self::InvalidSession => (StatusCode::UNAUTHORIZED, "invalid_session"),
            Self::SessionStore(_) | Self::Entropy(_) => (StatusCode::INTERNAL_SERVER_ERROR, "session_store_failure"),
        }
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let (status, code) = self.status_and_code();
        let logged: Option<&dyn std::error::Error> = status.is_server_error().then_some(&self);
        deny(status, code, logged)
    }
}

#[cfg(test)]
mod tests;
