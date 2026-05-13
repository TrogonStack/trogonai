use std::net::SocketAddr;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use tracing::info;

use crate::{
    store::{MemoryClient, MemoryStore},
    types::EntityMemory,
};

// ── AppState ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState<S: MemoryStore> {
    client: MemoryClient<S>,
}

// ── Public entry point ────────────────────────────────────────────────────────

#[cfg(not(coverage))]
pub async fn serve(
    port: u16,
    store: async_nats::jetstream::kv::Store,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    serve_impl(port, store).await
}

async fn serve_impl<S: MemoryStore>(
    port: u16,
    store: S,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = AppState { client: MemoryClient::new(store) };

    let app = Router::new()
        .route("/memory/{actor_type}/{*actor_key}", get(handle_get::<S>).delete(handle_delete::<S>))
        .route("/health", get(handle_health))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!(addr = %addr, "Memory management API listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("Memory management API shut down");
    Ok(())
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut sigterm = signal(SignalKind::terminate()).expect("failed to register SIGTERM");
    let mut sigint = signal(SignalKind::interrupt()).expect("failed to register SIGINT");
    tokio::select! {
        _ = sigterm.recv() => { info!("Received SIGTERM, shutting down"); }
        _ = sigint.recv()  => { info!("Received SIGINT, shutting down"); }
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn handle_health() -> StatusCode {
    StatusCode::OK
}

async fn handle_get<S: MemoryStore>(
    State(state): State<AppState<S>>,
    Path((actor_type, actor_key)): Path<(String, String)>,
) -> (StatusCode, Json<Option<EntityMemory>>) {
    match state.client.get(&actor_type, &actor_key).await {
        Ok(Some(memory)) => (StatusCode::OK, Json(Some(memory))),
        Ok(None) => (StatusCode::NOT_FOUND, Json(None)),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(None)),
    }
}

async fn handle_delete<S: MemoryStore>(
    State(state): State<AppState<S>>,
    Path((actor_type, actor_key)): Path<(String, String)>,
) -> StatusCode {
    match state.client.delete(&actor_type, &actor_key).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use crate::store::mock::MockMemoryStore;
    use crate::types::RawFact;
    use tower::util::ServiceExt as _;

    fn make_app() -> (Router, MockMemoryStore) {
        let store = MockMemoryStore::new();
        let state = AppState { client: MemoryClient::new(store.clone()) };
        let app = Router::new()
            .route(
                "/memory/{actor_type}/{*actor_key}",
                get(handle_get::<MockMemoryStore>).delete(handle_delete::<MockMemoryStore>),
            )
            .route("/health", get(handle_health))
            .with_state(state);
        (app, store)
    }

    async fn seed_memory(store: &MockMemoryStore, actor_type: &str, actor_key: &str) {
        let client = MemoryClient::new(store.clone());
        let mut memory = EntityMemory::default();
        memory.merge(
            vec![RawFact { category: "fact".into(), content: "uses Rust".into(), confidence: 0.9 }],
            "sess-1",
        );
        client.put(actor_type, actor_key, &memory).await.unwrap();
    }

    #[tokio::test]
    async fn health_returns_200() {
        let (app, _) = make_app();
        let resp = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn get_returns_memory_when_present() {
        let (app, store) = make_app();
        seed_memory(&store, "pr", "owner/repo/1").await;

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/memory/pr/owner/repo/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let memory: EntityMemory = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(memory.facts.len(), 1);
        assert_eq!(memory.facts[0].content, "uses Rust");
    }

    #[tokio::test]
    async fn get_returns_404_when_no_memory() {
        let (app, _) = make_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/memory/pr/unknown/entity")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_returns_204() {
        let (app, store) = make_app();
        seed_memory(&store, "pr", "owner/repo/1").await;

        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/memory/pr/owner/repo/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn delete_removes_memory_so_get_returns_404() {
        let store = MockMemoryStore::new();
        let state = AppState { client: MemoryClient::new(store.clone()) };
        let app = Router::new()
            .route(
                "/memory/{actor_type}/{*actor_key}",
                get(handle_get::<MockMemoryStore>).delete(handle_delete::<MockMemoryStore>),
            )
            .with_state(state);

        seed_memory(&store, "pr", "repo/1").await;

        // DELETE
        app.clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/memory/pr/repo/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // GET should now be 404
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/memory/pr/repo/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn actor_key_with_slashes_is_handled() {
        let (app, store) = make_app();
        seed_memory(&store, "pr", "org/repo/42").await;

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/memory/pr/org/repo/42")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }
}
