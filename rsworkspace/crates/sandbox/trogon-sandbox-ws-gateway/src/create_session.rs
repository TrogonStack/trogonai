use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use trogon_sandbox_auth::BearerToken;

use crate::constants::API_KEY_HEADER;
use crate::session_record::now_secs;
use crate::session_token::{mint_session_token, session_token_hash};
use crate::{AppState, CreateSessionResponse, HttpError, SessionRecord};

/// Exchanges an API key for a short-lived session token.
pub async fn create_session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<CreateSessionResponse>), HttpError> {
    let api_key = headers
        .get(API_KEY_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or(HttpError::MissingApiKey)?;
    let api_key = BearerToken::new(api_key).map_err(|_| HttpError::InvalidApiKey)?;
    let record = state
        .api_keys
        .lookup(&api_key.hash())
        .await?
        .ok_or(HttpError::InvalidApiKey)?;

    let token = mint_session_token()?;
    let expires_at = now_secs().saturating_add(state.session_ttl.as_secs());
    let session = SessionRecord {
        tenant: record.tenant().clone(),
        service_account: record.service_account().clone(),
        api_key: api_key.expose_secret().to_string(),
        expires_at,
    };
    state.sessions.put(&session_token_hash(&token)?, session).await?;
    Ok((StatusCode::CREATED, Json(CreateSessionResponse { token, expires_at })))
}
