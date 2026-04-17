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
    models::skill::{CreateSkillRequest, CreateSkillVersionRequest, Skill, SkillVersion},
    server::AppState,
};

fn now_version() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = t.as_secs();
    // Format as YYYYMMDD
    let days = secs / 86400;
    let epoch_days_to_2000: u64 = 10957;
    let days_since_2000 = days.saturating_sub(epoch_days_to_2000);
    let year = 2000 + days_since_2000 / 365;
    let rem = days_since_2000 % 365;
    let month = rem / 30 + 1;
    let day = rem % 30 + 1;
    format!("{year:04}{month:02}{day:02}")
}

fn now_iso() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

pub async fn list_skills(State(state): State<Arc<AppState>>) -> Result<impl IntoResponse, AppError> {
    let skills = state.skills.list().await.map_err(AppError::Store)?;
    Ok(Json(skills))
}

pub async fn get_skill(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    state
        .skills
        .get(&id)
        .await
        .map_err(AppError::Store)?
        .map(Json)
        .ok_or_else(|| AppError::NotFound(format!("skill {id} not found")))
}

pub async fn create_skill(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateSkillRequest>,
) -> Result<impl IntoResponse, AppError> {
    let now = now_iso();
    let version = now_version();
    let skill_id = format!("skill_{}", Uuid::new_v4().simple());

    let skill = Skill {
        id: skill_id.clone(),
        name: req.name,
        description: req.description,
        provider: req.provider,
        latest_version: version.clone(),
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    state.skills.put(&skill).await.map_err(AppError::Store)?;

    let sv = SkillVersion {
        skill_id: skill_id.clone(),
        version: version.clone(),
        content: req.content,
        is_latest: true,
        created_at: now,
    };
    state.skills.put_version(&sv).await.map_err(AppError::Store)?;

    Ok((StatusCode::CREATED, Json(skill)))
}

pub async fn list_skill_versions(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let versions = state.skills.list_versions(&id).await.map_err(AppError::Store)?;
    Ok(Json(versions))
}

pub async fn create_skill_version(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<CreateSkillVersionRequest>,
) -> Result<impl IntoResponse, AppError> {
    let mut skill = state
        .skills
        .get(&id)
        .await
        .map_err(AppError::Store)?
        .ok_or_else(|| AppError::NotFound(format!("skill {id} not found")))?;

    let now = now_iso();
    let version = now_version();

    let sv = SkillVersion {
        skill_id: id.clone(),
        version: version.clone(),
        content: req.content,
        is_latest: true,
        created_at: now.clone(),
    };
    state.skills.put_version(&sv).await.map_err(AppError::Store)?;

    skill.latest_version = version;
    skill.updated_at = now;
    state.skills.put(&skill).await.map_err(AppError::Store)?;

    Ok((StatusCode::CREATED, Json(sv)))
}
