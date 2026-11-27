use axum::http::StatusCode;
use trogon_sandbox_auth::{ApiKeyStoreError, BearerTokenError};

use super::HttpError;
use crate::{EntropyError, SessionStoreError};

fn status_of(error: HttpError) -> (StatusCode, &'static str) {
    error.status_and_code()
}

fn record_error() -> serde_json::Error {
    <serde_json::Error as serde::de::Error>::custom("bad record")
}

#[test]
fn client_mistakes_are_unauthorized() {
    assert_eq!(
        status_of(HttpError::MissingApiKey),
        (StatusCode::UNAUTHORIZED, "missing_api_key")
    );
    assert_eq!(
        status_of(HttpError::InvalidApiKey),
        (StatusCode::UNAUTHORIZED, "invalid_api_key")
    );
    assert_eq!(
        status_of(HttpError::MissingToken),
        (StatusCode::UNAUTHORIZED, "missing_or_invalid_token")
    );
    assert_eq!(
        status_of(HttpError::InvalidToken(BearerTokenError::Empty)),
        (StatusCode::UNAUTHORIZED, "missing_or_invalid_token")
    );
    assert_eq!(
        status_of(HttpError::InvalidSession),
        (StatusCode::UNAUTHORIZED, "invalid_session")
    );
}

#[test]
fn backend_failures_are_server_errors() {
    assert_eq!(
        status_of(HttpError::ApiKeyBackend(ApiKeyStoreError::Record(record_error()))),
        (StatusCode::INTERNAL_SERVER_ERROR, "api_key_backend_failure")
    );
    assert_eq!(
        status_of(HttpError::SessionStore(SessionStoreError::Record(record_error()))),
        (StatusCode::INTERNAL_SERVER_ERROR, "session_store_failure")
    );
    assert_eq!(
        status_of(HttpError::Entropy(EntropyError::from(rand_core::Error::new(
            std::io::Error::other("no entropy")
        )))),
        (StatusCode::INTERNAL_SERVER_ERROR, "session_store_failure")
    );
}
