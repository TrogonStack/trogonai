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
    models::environment::{CreateEnvironmentRequest, Environment, UpdateEnvironmentRequest},
    server::AppState,
};

fn now_iso() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

pub async fn list_environments(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    let envs = state.environments.list().await.map_err(AppError::Store)?;
    Ok(Json(envs))
}

pub async fn get_environment(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    state
        .environments
        .get(&id)
        .await
        .map_err(AppError::Store)?
        .map(Json)
        .ok_or_else(|| AppError::NotFound(format!("environment {id} not found")))
}

pub async fn create_environment(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateEnvironmentRequest>,
) -> Result<impl IntoResponse, AppError> {
    let now = now_iso();
    let env = Environment {
        id: format!("env_{}", Uuid::new_v4().simple()),
        name: req.name,
        description: req.description,
        env_type: req.env_type,
        networking: req.networking,
        packages: req.packages,
        metadata: req.metadata,
        archived: false,
        created_at: now.clone(),
        updated_at: now,
    };
    state
        .environments
        .put(&env)
        .await
        .map_err(AppError::Store)?;
    Ok((StatusCode::CREATED, Json(env)))
}

pub async fn update_environment(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateEnvironmentRequest>,
) -> Result<impl IntoResponse, AppError> {
    let mut env = state
        .environments
        .get(&id)
        .await
        .map_err(AppError::Store)?
        .ok_or_else(|| AppError::NotFound(format!("environment {id} not found")))?;

    if let Some(name) = req.name {
        env.name = name;
    }
    if let Some(desc) = req.description {
        env.description = desc;
    }
    if let Some(t) = req.env_type {
        env.env_type = t;
    }
    if let Some(net) = req.networking {
        env.networking = net;
    }
    if let Some(pkgs) = req.packages {
        env.packages = pkgs;
    }
    if let Some(meta) = req.metadata {
        env.metadata = meta;
    }

    env.updated_at = now_iso();
    state
        .environments
        .put(&env)
        .await
        .map_err(AppError::Store)?;
    Ok(Json(env))
}

pub async fn delete_environment(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    state
        .environments
        .delete(&id)
        .await
        .map_err(AppError::Store)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn archive_environment(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let mut env = state
        .environments
        .get(&id)
        .await
        .map_err(AppError::Store)?
        .ok_or_else(|| AppError::NotFound(format!("environment {id} not found")))?;

    env.archived = true;
    env.updated_at = now_iso();
    state
        .environments
        .put(&env)
        .await
        .map_err(AppError::Store)?;
    Ok(Json(env))
}
