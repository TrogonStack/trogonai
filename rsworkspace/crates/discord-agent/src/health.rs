//! Health check endpoint for the Discord agent.

use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

/// Lightweight counters shared between the processor and the health endpoint.
#[derive(Clone, Default)]
pub struct AgentMetrics {
    pub messages_processed: Arc<AtomicU64>,
    pub llm_errors: Arc<AtomicU64>,
}

impl AgentMetrics {
    pub fn inc_messages_processed(&self) {
        self.messages_processed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_llm_errors(&self) {
        self.llm_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn messages_processed(&self) -> u64 {
        self.messages_processed.load(Ordering::Relaxed)
    }

    pub fn llm_errors(&self) -> u64 {
        self.llm_errors.load(Ordering::Relaxed)
    }
}

/// Health check response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub status: String,
    pub agent_name: String,
    pub mode: String,
    pub uptime_secs: u64,
    pub messages_processed: u64,
    pub llm_errors: u64,
}

/// Shared application state for health checks
#[derive(Clone)]
pub struct AgentHealthState {
    pub start_time: SystemTime,
    pub agent_name: Arc<String>,
    pub mode: Arc<String>,
    pub metrics: AgentMetrics,
}

impl AgentHealthState {
    pub fn new(agent_name: String, mode: String) -> Self {
        Self {
            start_time: SystemTime::now(),
            agent_name: Arc::new(agent_name),
            mode: Arc::new(mode),
            metrics: AgentMetrics::default(),
        }
    }
}

async fn health_handler(State(state): State<AgentHealthState>) -> (StatusCode, Json<HealthStatus>) {
    let uptime = state.start_time.elapsed().unwrap_or_default().as_secs();
    (
        StatusCode::OK,
        Json(HealthStatus {
            status: "ok".to_string(),
            agent_name: (*state.agent_name).clone(),
            mode: (*state.mode).clone(),
            uptime_secs: uptime,
            messages_processed: state.metrics.messages_processed(),
            llm_errors: state.metrics.llm_errors(),
        }),
    )
}

async fn live_handler() -> StatusCode {
    StatusCode::NO_CONTENT
}

/// Start the health check server
pub async fn start_health_server(state: AgentHealthState, port: u16) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/live", get(live_handler))
        .with_state(state);
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Agent health server listening on {}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_status_serde() {
        let status = HealthStatus {
            status: "ok".to_string(),
            agent_name: "discord-agent-llm".to_string(),
            mode: "llm".to_string(),
            uptime_secs: 42,
            messages_processed: 100,
            llm_errors: 2,
        };
        let json = serde_json::to_string(&status).unwrap();
        let back: HealthStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, "ok");
        assert_eq!(back.agent_name, "discord-agent-llm");
        assert_eq!(back.mode, "llm");
        assert_eq!(back.uptime_secs, 42);
        assert_eq!(back.messages_processed, 100);
        assert_eq!(back.llm_errors, 2);
    }

    #[test]
    fn test_agent_health_state_new() {
        let state = AgentHealthState::new("my-agent".to_string(), "echo".to_string());
        assert_eq!(*state.agent_name, "my-agent");
        assert_eq!(*state.mode, "echo");
        assert_eq!(state.metrics.messages_processed(), 0);
        assert_eq!(state.metrics.llm_errors(), 0);
    }

    #[test]
    fn test_agent_metrics_increment() {
        let m = AgentMetrics::default();
        m.inc_messages_processed();
        m.inc_messages_processed();
        m.inc_llm_errors();
        assert_eq!(m.messages_processed(), 2);
        assert_eq!(m.llm_errors(), 1);
    }

    #[test]
    fn test_agent_metrics_clone_shares_state() {
        let m = AgentMetrics::default();
        let m2 = m.clone();
        m.inc_messages_processed();
        // Clone shares the Arc, so both see the same value
        assert_eq!(m2.messages_processed(), 1);
    }
}
