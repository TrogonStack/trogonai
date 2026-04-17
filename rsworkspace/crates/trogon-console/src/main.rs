mod error;
mod models;
mod routes;
mod server;
mod store;

use std::sync::Arc;
use tracing::info;

use store::{
    agents::AgentStore, credentials::CredentialStore, environments::EnvironmentStore,
    sessions::SessionReader, skills::SkillStore,
};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "trogon_console=info".parse().unwrap()),
        )
        .init();

    if let Err(e) = run().await {
        eprintln!("trogon-console error: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let nats_url =
        std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string());
    let port: u16 = std::env::var("CONSOLE_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8090);

    info!("connecting to NATS at {nats_url}");
    let nats = async_nats::connect(&nats_url)
        .await
        .map_err(|e| e.to_string())?;
    let js = async_nats::jetstream::new(nats);

    let state = Arc::new(server::AppState {
        agents: AgentStore::open(&js).await?,
        skills: SkillStore::open(&js).await?,
        environments: EnvironmentStore::open(&js).await?,
        credentials: CredentialStore::open(&js).await?,
        sessions: SessionReader::open(&js).await?,
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
