pub mod error;
pub mod models;
pub mod routes;
pub mod server;
pub mod store;

use std::sync::Arc;
use tracing::info;

use store::{
    agents::AgentStore,
    credentials::CredentialStore,
    environments::EnvironmentStore,
    sessions::SessionReader,
    skills::SkillStore,
    traits::{
        AgentRepository, CredentialRepository, EnvironmentRepository, SessionRepository,
        SkillRepository,
    },
};

pub async fn run() -> Result<(), String> {
    let nats_url =
        std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string());
    let port: u16 = std::env::var("CONSOLE_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8090);
    run_with(&nats_url, port).await
}

pub async fn run_with(nats_url: &str, port: u16) -> Result<(), String> {
    info!("connecting to NATS at {nats_url}");
    let nats = async_nats::connect(nats_url)
        .await
        .map_err(|e| e.to_string())?;
    let js = async_nats::jetstream::new(nats);

    let state = Arc::new(server::AppState {
        agents: Arc::new(AgentStore::open(&js).await?) as Arc<dyn AgentRepository>,
        skills: Arc::new(SkillStore::open(&js).await?) as Arc<dyn SkillRepository>,
        environments: Arc::new(EnvironmentStore::open(&js).await?)
            as Arc<dyn EnvironmentRepository>,
        credentials: Arc::new(CredentialStore::open(&js).await?) as Arc<dyn CredentialRepository>,
        sessions: Arc::new(SessionReader::open(&js).await?) as Arc<dyn SessionRepository>,
    });

    let router = server::build_router(state);
    let addr = format!("0.0.0.0:{port}");
    info!("trogon-console listening on {addr}");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| e.to_string())?;
    axum::serve(listener, router)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn run_bad_nats_returns_error() {
        unsafe {
            std::env::set_var("NATS_URL", "nats://127.0.0.1:1");
            std::env::set_var("CONSOLE_PORT", "0");
        }
        let result = run().await;
        unsafe {
            std::env::remove_var("NATS_URL");
            std::env::remove_var("CONSOLE_PORT");
        }
        assert!(result.is_err());
    }
}
