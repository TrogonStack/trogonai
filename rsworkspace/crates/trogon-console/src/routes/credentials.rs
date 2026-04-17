use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    error::AppError,
    models::credential::{Credential, CredentialStatus, CreateCredentialRequest},
    server::AppState,
};

fn now_iso() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

pub async fn get_vault(
    State(state): State<Arc<AppState>>,
    Path(env_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let vault = state
        .credentials
        .get_vault(&env_id)
        .await
        .map_err(AppError::Store)?
        .ok_or_else(|| AppError::NotFound(format!("vault for environment {env_id} not found")))?;
    Ok(Json(vault))
}

pub async fn list_credentials(
    State(state): State<Arc<AppState>>,
    Path(env_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let creds = state.credentials.list(&env_id).await.map_err(AppError::Store)?;
    Ok(Json(creds))
}

pub async fn create_credential(
    State(state): State<Arc<AppState>>,
    Path(env_id): Path<String>,
    Json(req): Json<CreateCredentialRequest>,
) -> Result<impl IntoResponse, AppError> {
    let vault = state
        .credentials
        .get_or_create_vault(&env_id)
        .await
        .map_err(AppError::Store)?;

    let now = now_iso();
    let cred = Credential {
        id: format!("crd_{}", Uuid::new_v4().simple()),
        vault_id: vault.id,
        env_id,
        name: req.name,
        credential_type: req.credential_type,
        mcp_server_url: req.mcp_server_url,
        status: CredentialStatus::Active,
        rotation_policy_days: req.rotation_policy_days,
        created_at: now.clone(),
        updated_at: now,
    };
    state.credentials.put(&cred).await.map_err(AppError::Store)?;
    Ok((StatusCode::CREATED, Json(cred)))
}

pub async fn delete_credential(
    State(state): State<Arc<AppState>>,
    Path((env_id, cred_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    state
        .credentials
        .delete(&env_id, &cred_id)
        .await
        .map_err(AppError::Store)?;
    Ok(StatusCode::NO_CONTENT)
}
