use std::net::SocketAddr;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use serde::Deserialize;
use tracing::info;
use uuid::Uuid;

use crate::registry::WebhookRegistry;
use crate::store::SubscriptionStore;
use crate::subscription::WebhookSubscription;

// ── Application state ─────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState<S: SubscriptionStore> {
    registry: WebhookRegistry<S>,
}

// ── Request / response types ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct RegisterRequest {
    subject_pattern: String,
    url: String,
    secret: Option<String>,
}

// ── Public entry-point ────────────────────────────────────────────────────────

#[cfg(not(coverage))]
pub async fn serve(
    port: u16,
    registry: WebhookRegistry<async_nats::jetstream::kv::Store>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    serve_impl(port, registry).await
}

async fn serve_impl<S: SubscriptionStore>(
    port: u16,
    registry: WebhookRegistry<S>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = AppState { registry };

    let app = Router::new()
        .route(
            "/subscriptions",
            post(handle_register::<S>).get(handle_list::<S>),
        )
        .route("/subscriptions/{id}", delete(handle_deregister::<S>))
        .route("/health", get(handle_health))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!(addr = %addr, "Webhook dispatcher management API listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("Webhook dispatcher management API shut down");
    Ok(())
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigterm = signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
    let mut sigint = signal(SignalKind::interrupt()).expect("failed to register SIGINT handler");

    tokio::select! {
        _ = sigterm.recv() => { info!("Received SIGTERM, shutting down"); }
        _ = sigint.recv()  => { info!("Received SIGINT, shutting down"); }
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn handle_health() -> StatusCode {
    StatusCode::OK
}

async fn handle_register<S: SubscriptionStore>(
    State(state): State<AppState<S>>,
    Json(body): Json<RegisterRequest>,
) -> (StatusCode, Json<WebhookSubscription>) {
    let sub = WebhookSubscription {
        id: Uuid::new_v4().to_string(),
        subject_pattern: body.subject_pattern,
        url: body.url,
        secret: body.secret,
    };

    match state.registry.register(&sub).await {
        Ok(()) => (StatusCode::CREATED, Json(sub)),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(sub)),
    }
}

async fn handle_list<S: SubscriptionStore>(
    State(state): State<AppState<S>>,
) -> (StatusCode, Json<Vec<WebhookSubscription>>) {
    match state.registry.list().await {
        Ok(subs) => (StatusCode::OK, Json(subs)),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(vec![])),
    }
}

async fn handle_deregister<S: SubscriptionStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
) -> StatusCode {
    match state.registry.deregister(&id).await {
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
    use crate::store::mock::MockSubscriptionStore;
    use tower::util::ServiceExt as _;

    fn make_app() -> Router {
        let registry = WebhookRegistry::new(MockSubscriptionStore::new());
        let state = AppState { registry };
        Router::new()
            .route(
                "/subscriptions",
                post(handle_register::<MockSubscriptionStore>)
                    .get(handle_list::<MockSubscriptionStore>),
            )
            .route(
                "/subscriptions/{id}",
                delete(handle_deregister::<MockSubscriptionStore>),
            )
            .route("/health", get(handle_health))
            .with_state(state)
    }

    #[tokio::test]
    async fn health_returns_200() {
        let app = make_app();
        let req = Request::builder()
            .method("GET")
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn register_returns_201_with_subscription() {
        let app = make_app();
        let body = serde_json::json!({
            "subject_pattern": "transcripts.>",
            "url": "https://example.com/hook"
        });
        let req = Request::builder()
            .method("POST")
            .uri("/subscriptions")
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let sub: WebhookSubscription = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(sub.subject_pattern, "transcripts.>");
        assert_eq!(sub.url, "https://example.com/hook");
        assert!(!sub.id.is_empty());
    }

    #[tokio::test]
    async fn list_returns_registered_subscriptions() {
        let registry = WebhookRegistry::new(MockSubscriptionStore::new());
        registry
            .register(&WebhookSubscription {
                id: "pre-existing".to_string(),
                subject_pattern: "github.>".to_string(),
                url: "https://example.com".to_string(),
                secret: None,
            })
            .await
            .unwrap();

        let state = AppState { registry };
        let app = Router::new()
            .route("/subscriptions", get(handle_list::<MockSubscriptionStore>))
            .with_state(state);

        let req = Request::builder()
            .method("GET")
            .uri("/subscriptions")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let subs: Vec<WebhookSubscription> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].id, "pre-existing");
    }

    #[tokio::test]
    async fn deregister_returns_204() {
        let registry = WebhookRegistry::new(MockSubscriptionStore::new());
        registry
            .register(&WebhookSubscription {
                id: "to-delete".to_string(),
                subject_pattern: ">".to_string(),
                url: "https://example.com".to_string(),
                secret: None,
            })
            .await
            .unwrap();

        let state = AppState { registry };
        let app = Router::new()
            .route(
                "/subscriptions/{id}",
                delete(handle_deregister::<MockSubscriptionStore>),
            )
            .with_state(state);

        let req = Request::builder()
            .method("DELETE")
            .uri("/subscriptions/to-delete")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn register_with_secret() {
        let app = make_app();
        let body = serde_json::json!({
            "subject_pattern": "transcripts.>",
            "url": "https://example.com/hook",
            "secret": "my-secret"
        });
        let req = Request::builder()
            .method("POST")
            .uri("/subscriptions")
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let sub: WebhookSubscription = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(sub.secret.as_deref(), Some("my-secret"));
    }
}
