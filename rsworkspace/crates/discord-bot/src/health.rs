//! Health check endpoint

use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use serenity::prelude::TypeMapKey;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;

/// Health check response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub status: String,
    pub bot_username: Option<String>,
    pub uptime_secs: u64,
}

/// Shared application state for health checks
#[derive(Clone)]
pub struct AppState {
    pub start_time: SystemTime,
    pub bot_username: Arc<RwLock<Option<String>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            start_time: SystemTime::now(),
            bot_username: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn set_bot_username(&self, username: String) {
        let mut guard = self.bot_username.write().await;
        *guard = Some(username);
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeMapKey for AppState {
    type Value = AppState;
}

async fn health_handler(State(state): State<AppState>) -> (StatusCode, Json<HealthStatus>) {
    let uptime = state.start_time.elapsed().unwrap_or_default().as_secs();
    let bot_username = state.bot_username.read().await.clone();

    (
        StatusCode::OK,
        Json(HealthStatus {
            status: "ok".to_string(),
            bot_username,
            uptime_secs: uptime,
        }),
    )
}

async fn live_handler() -> StatusCode {
    StatusCode::OK
}

/// Create the health check router
pub fn create_health_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/live", get(live_handler))
        .with_state(state)
}

/// Start the health check server
pub async fn start_health_server(state: AppState, port: u16) -> anyhow::Result<()> {
    let app = create_health_router(state);
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Health check server listening on {}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_app_state_new() {
        let state = AppState::new();
        assert!(state.bot_username.read().await.is_none());
    }

    #[tokio::test]
    async fn test_set_bot_username() {
        let state = AppState::new();
        state.set_bot_username("mybot".to_string()).await;
        assert_eq!(*state.bot_username.read().await, Some("mybot".to_string()));
    }

    #[test]
    fn test_health_status_serde() {
        let status = HealthStatus {
            status: "ok".to_string(),
            bot_username: Some("testbot".to_string()),
            uptime_secs: 100,
        };
        let json = serde_json::to_string(&status).unwrap();
        let back: HealthStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, "ok");
        assert_eq!(back.uptime_secs, 100);
        assert_eq!(back.bot_username, Some("testbot".to_string()));
    }
}
