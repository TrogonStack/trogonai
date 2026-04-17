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

fn is_leap_year(year: u64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Returns today's date as `YYYYMMDD` — used as a version key for skill versions.
fn now_version() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut remaining = secs / 86400;
    let mut year = 1970u64;
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }
    let days_in_month = [
        31u64,
        if is_leap_year(year) { 29 } else { 28 },
        31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
    ];
    let mut month = 1u64;
    for &dim in &days_in_month {
        if remaining < dim {
            break;
        }
        remaining -= dim;
        month += 1;
    }
    let day = remaining + 1;
    format!("{year:04}{month:02}{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_version_format_is_valid_yyyymmdd() {
        let v = now_version();
        assert_eq!(v.len(), 8, "version must be 8 digits: {v}");
        assert!(v.chars().all(|c| c.is_ascii_digit()), "version must be all digits: {v}");
        let year: u32 = v[0..4].parse().unwrap();
        let month: u32 = v[4..6].parse().unwrap();
        let day: u32 = v[6..8].parse().unwrap();
        assert!(year >= 2025, "year must be >= 2025: {v}");
        assert!((1..=12).contains(&month), "month must be 1-12: {v}");
        assert!((1..=31).contains(&day), "day must be 1-31: {v}");
    }

    #[test]
    fn version_for_known_epoch_is_correct() {
        // 2026-04-17T00:00:00Z:
        //   days to 2026-01-01: 20454 (accounting for all leap years 1970-2025)
        //   days Jan(31)+Feb(28)+Mar(31)+16 = 106 days into 2026
        //   total: 20560 days * 86400 = 1776384000 secs
        let secs: u64 = 1776384000; // 2026-04-17T00:00:00Z
        let mut remaining = secs / 86400;
        let mut year = 1970u64;
        loop {
            let days_in_year = if is_leap_year(year) { 366 } else { 365 };
            if remaining < days_in_year { break; }
            remaining -= days_in_year;
            year += 1;
        }
        let days_in_month = [
            31u64,
            if is_leap_year(year) { 29 } else { 28 },
            31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
        ];
        let mut month = 1u64;
        for &dim in &days_in_month {
            if remaining < dim { break; }
            remaining -= dim;
            month += 1;
        }
        let day = remaining + 1;
        assert_eq!(year, 2026);
        assert_eq!(month, 4);
        assert_eq!(day, 17);
    }
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
