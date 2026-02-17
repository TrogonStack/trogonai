//! Health check and metrics endpoint

use axum::{
    extract::State,
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

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

    pub async fn set_active_sessions(&self, count: usize) {
        let mut metrics = self.metrics.write().await;
        metrics.active_sessions = count;
    }
}

/// Health check endpoint handler
async fn health_handler(State(state): State<AppState>) -> (StatusCode, Json<HealthStatus>) {
    let uptime = state.start_time
        .elapsed()
        .unwrap_or_default()
        .as_secs();

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

/// Start health check server
pub async fn start_health_server(state: AppState, port: u16) -> anyhow::Result<()> {
    let app = create_health_router(state);

    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    tracing::info!("Health check server listening on {}", addr);

    axum::serve(listener, app).await?;

    Ok(())
}
