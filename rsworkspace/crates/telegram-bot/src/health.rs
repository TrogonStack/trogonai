//! Health check and metrics endpoint

use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;

/// Health check status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub status: String,
    pub uptime_seconds: u64,
    pub nats_connected: bool,
    pub bot_username: Option<String>,
}

/// Metrics data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metrics {
    pub messages_received: u64,
    pub messages_sent: u64,
    pub commands_processed: u64,
    pub active_sessions: usize,
    pub errors: u64,
}

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub metrics: Arc<RwLock<Metrics>>,
    pub start_time: SystemTime,
    pub bot_username: Option<String>,
    pub nats_connected: Arc<RwLock<bool>>,
}

impl AppState {
    pub fn new(bot_username: Option<String>) -> Self {
        Self {
            metrics: Arc::new(RwLock::new(Metrics {
                messages_received: 0,
                messages_sent: 0,
                commands_processed: 0,
                active_sessions: 0,
                errors: 0,
            })),
            start_time: SystemTime::now(),
            bot_username,
            nats_connected: Arc::new(RwLock::new(false)),
        }
    }

    pub async fn increment_messages_received(&self) {
        let mut metrics = self.metrics.write().await;
        metrics.messages_received += 1;
    }

    #[allow(dead_code)]
    pub async fn increment_messages_sent(&self) {
        let mut metrics = self.metrics.write().await;
        metrics.messages_sent += 1;
    }

    pub async fn increment_commands(&self) {
        let mut metrics = self.metrics.write().await;
        metrics.commands_processed += 1;
    }

    pub async fn increment_errors(&self) {
        let mut metrics = self.metrics.write().await;
        metrics.errors += 1;
    }

    #[allow(dead_code)]
    pub async fn set_active_sessions(&self, count: usize) {
        let mut metrics = self.metrics.write().await;
        metrics.active_sessions = count;
    }
}

/// Health check endpoint handler
async fn health_handler(State(state): State<AppState>) -> (StatusCode, Json<HealthStatus>) {
    let uptime = state.start_time.elapsed().unwrap_or_default().as_secs();

    let nats_connected = *state.nats_connected.read().await;

    let status = if nats_connected {
        "healthy"
    } else {
        "unhealthy"
    };

    let status_code = if nats_connected {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status_code,
        Json(HealthStatus {
            status: status.to_string(),
            uptime_seconds: uptime,
            nats_connected,
            bot_username: state.bot_username.clone(),
        }),
    )
}

/// Metrics endpoint handler
async fn metrics_handler(State(state): State<AppState>) -> Json<Metrics> {
    let metrics = state.metrics.read().await;
    Json(metrics.clone())
}

/// Create health check router
pub fn create_health_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/metrics", get(metrics_handler))
        .route("/ready", get(ready_handler))
        .route("/live", get(live_handler))
        .with_state(state)
}

/// Readiness check (ready to accept traffic)
async fn ready_handler(State(state): State<AppState>) -> StatusCode {
    let nats_connected = *state.nats_connected.read().await;

    if nats_connected {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

/// Liveness check (process is alive)
async fn live_handler() -> StatusCode {
    StatusCode::OK
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── AppState::new() ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_new_initializes_counters_to_zero() {
        let state = AppState::new(None);
        let metrics = state.metrics.read().await;
        assert_eq!(metrics.messages_received, 0);
        assert_eq!(metrics.messages_sent, 0);
        assert_eq!(metrics.commands_processed, 0);
        assert_eq!(metrics.active_sessions, 0);
        assert_eq!(metrics.errors, 0);
    }

    #[tokio::test]
    async fn test_new_stores_bot_username() {
        let state = AppState::new(Some("mybot".to_string()));
        assert_eq!(state.bot_username, Some("mybot".to_string()));
    }

    #[tokio::test]
    async fn test_new_nats_not_connected() {
        let state = AppState::new(None);
        assert!(!*state.nats_connected.read().await);
    }

    // ── Counter increments ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_increment_messages_received() {
        let state = AppState::new(None);
        state.increment_messages_received().await;
        state.increment_messages_received().await;
        state.increment_messages_received().await;
        let metrics = state.metrics.read().await;
        assert_eq!(metrics.messages_received, 3);
    }

    #[tokio::test]
    async fn test_increment_messages_sent() {
        let state = AppState::new(None);
        state.increment_messages_sent().await;
        state.increment_messages_sent().await;
        let metrics = state.metrics.read().await;
        assert_eq!(metrics.messages_sent, 2);
    }

    #[tokio::test]
    async fn test_increment_commands() {
        let state = AppState::new(None);
        state.increment_commands().await;
        let metrics = state.metrics.read().await;
        assert_eq!(metrics.commands_processed, 1);
    }

    #[tokio::test]
    async fn test_increment_errors() {
        let state = AppState::new(None);
        state.increment_errors().await;
        state.increment_errors().await;
        state.increment_errors().await;
        state.increment_errors().await;
        let metrics = state.metrics.read().await;
        assert_eq!(metrics.errors, 4);
    }

    #[tokio::test]
    async fn test_set_active_sessions() {
        let state = AppState::new(None);
        state.set_active_sessions(42).await;
        let metrics = state.metrics.read().await;
        assert_eq!(metrics.active_sessions, 42);
    }

    #[tokio::test]
    async fn test_counters_are_independent() {
        let state = AppState::new(None);
        state.increment_messages_received().await;
        state.increment_messages_received().await;
        state.increment_messages_sent().await;
        state.increment_errors().await;
        state.increment_commands().await;
        state.increment_commands().await;

        let metrics = state.metrics.read().await;
        assert_eq!(metrics.messages_received, 2);
        assert_eq!(metrics.messages_sent, 1);
        assert_eq!(metrics.errors, 1);
        assert_eq!(metrics.commands_processed, 2);
        assert_eq!(metrics.active_sessions, 0);
    }

    // ── HealthStatus serde ────────────────────────────────────────────────────

    #[test]
    fn test_health_status_serde() {
        let status = HealthStatus {
            status: "healthy".to_string(),
            uptime_seconds: 3600,
            nats_connected: true,
            bot_username: Some("testbot".to_string()),
        };
        let json = serde_json::to_string(&status).unwrap();
        let back: HealthStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, "healthy");
        assert_eq!(back.uptime_seconds, 3600);
        assert!(back.nats_connected);
        assert_eq!(back.bot_username, Some("testbot".to_string()));
    }

    #[test]
    fn test_health_status_unhealthy_serde() {
        let status = HealthStatus {
            status: "unhealthy".to_string(),
            uptime_seconds: 0,
            nats_connected: false,
            bot_username: None,
        };
        let json = serde_json::to_string(&status).unwrap();
        let back: HealthStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, "unhealthy");
        assert!(!back.nats_connected);
        assert!(back.bot_username.is_none());
    }

    // ── Metrics serde ─────────────────────────────────────────────────────────

    #[test]
    fn test_metrics_serde() {
        let metrics = Metrics {
            messages_received: 100,
            messages_sent: 95,
            commands_processed: 10,
            active_sessions: 5,
            errors: 2,
        };
        let json = serde_json::to_string(&metrics).unwrap();
        let back: Metrics = serde_json::from_str(&json).unwrap();
        assert_eq!(back.messages_received, 100);
        assert_eq!(back.messages_sent, 95);
        assert_eq!(back.commands_processed, 10);
        assert_eq!(back.active_sessions, 5);
        assert_eq!(back.errors, 2);
    }

    // ── Clone behavior ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_clone_shares_state() {
        let state1 = AppState::new(None);
        let state2 = state1.clone();

        state1.increment_messages_received().await;
        // Both clones share the same Arc, so state2 sees the change
        let metrics = state2.metrics.read().await;
        assert_eq!(metrics.messages_received, 1);
    }
}

/// Start health check server
pub async fn start_health_server(state: AppState, port: u16) -> anyhow::Result<()> {
    let app = create_health_router(state);

    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    tracing::info!("Health check server listening on {}", addr);

    axum::serve(listener, app).await?;

    Ok(())
}
