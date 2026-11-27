use std::sync::Arc;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Request, State};
use axum::response::Response;

use crate::constants::WS_TOKEN_PROTOCOL_PREFIX;
use crate::session_token::session_token_hash;
use crate::ws_token::extract_ws_token;
use crate::{AppState, HttpError, handle_socket};

/// Authenticates a socket request by its session token and serves the socket.
pub async fn ws_upgrade(
    State(state): State<AppState>,
    upgrade: WebSocketUpgrade,
    request: Request,
) -> Result<Response, HttpError> {
    let token = extract_ws_token(request.headers()).ok_or(HttpError::MissingToken)?;
    let session = state
        .sessions
        .lookup(&session_token_hash(&token)?)
        .await?
        .ok_or(HttpError::InvalidSession)?;

    let backend = Arc::clone(&state.backend);
    let sessions = Arc::clone(&state.sessions);
    let preview = state.preview.clone();
    let protocol = format!("{WS_TOKEN_PROTOCOL_PREFIX}{token}");
    Ok(upgrade
        .protocols([protocol])
        .on_upgrade(move |socket| handle_socket(socket, session, backend, sessions, preview)))
}
