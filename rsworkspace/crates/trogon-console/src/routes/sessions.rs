use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use std::sync::Arc;

use crate::{error::AppError, server::AppState};

pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    let sessions = state.sessions.list().await.map_err(AppError::Store)?;
    Ok(Json(sessions))
}

pub async fn get_session(
    State(state): State<Arc<AppState>>,
    Path((tenant_id, session_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    state
        .sessions
        .get(&tenant_id, &session_id)
        .await
        .map_err(AppError::Store)?
        .map(Json)
        .ok_or_else(|| AppError::NotFound(format!("session {session_id} not found")))
}
