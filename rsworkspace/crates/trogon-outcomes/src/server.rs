use std::net::SocketAddr;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::Deserialize;
use tracing::info;

use crate::{
    store::{OutcomesStore, ResultClient, RubricClient},
    types::{Criterion, EvaluationResult, Rubric},
};

// ── AppState ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState<S: OutcomesStore> {
    rubrics: RubricClient<S>,
    results: ResultClient<S>,
}

// ── Request types ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct CreateRubricRequest {
    name: String,
    description: String,
    actor_type_filter: Option<String>,
    criteria: Vec<Criterion>,
    passing_score: Option<f32>,
}

// ── Public entry point ────────────────────────────────────────────────────────

#[cfg(not(coverage))]
pub async fn serve(
    port: u16,
    rubric_store: async_nats::jetstream::kv::Store,
    result_store: async_nats::jetstream::kv::Store,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    serve_impl(port, rubric_store, result_store).await
}

async fn serve_impl<S: OutcomesStore>(
    port: u16,
    rubric_store: S,
    result_store: S,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = AppState {
        rubrics: RubricClient::new(rubric_store),
        results: ResultClient::new(result_store),
    };

    let app = Router::new()
        .route("/rubrics", post(handle_create_rubric::<S>).get(handle_list_rubrics::<S>))
        .route(
            "/rubrics/{id}",
            get(handle_get_rubric::<S>).delete(handle_delete_rubric::<S>),
        )
        .route("/evaluations/{session_id}", get(handle_list_results::<S>))
        .route("/health", get(handle_health))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!(addr = %addr, "Outcomes management API listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("Outcomes management API shut down");
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

async fn handle_create_rubric<S: OutcomesStore>(
    State(state): State<AppState<S>>,
    Json(body): Json<CreateRubricRequest>,
) -> (StatusCode, Json<Rubric>) {
    let mut rubric = Rubric::new(
        uuid::Uuid::new_v4().to_string(),
        body.name,
        body.description,
        body.criteria,
    );
    if let Some(filter) = body.actor_type_filter {
        rubric = rubric.with_actor_type_filter(filter);
    }
    if let Some(score) = body.passing_score {
        rubric = rubric.with_passing_score(score);
    }

    match state.rubrics.put(&rubric).await {
        Ok(()) => (StatusCode::CREATED, Json(rubric)),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(rubric)),
    }
}

async fn handle_list_rubrics<S: OutcomesStore>(
    State(state): State<AppState<S>>,
) -> (StatusCode, Json<Vec<Rubric>>) {
    match state.rubrics.list().await {
        Ok(rubrics) => (StatusCode::OK, Json(rubrics)),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(vec![])),
    }
}

async fn handle_get_rubric<S: OutcomesStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
) -> (StatusCode, Json<Option<Rubric>>) {
    match state.rubrics.get(&id).await {
        Ok(Some(r)) => (StatusCode::OK, Json(Some(r))),
        Ok(None) => (StatusCode::NOT_FOUND, Json(None)),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(None)),
    }
}

async fn handle_delete_rubric<S: OutcomesStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
) -> StatusCode {
    match state.rubrics.delete(&id).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn handle_list_results<S: OutcomesStore>(
    State(state): State<AppState<S>>,
    Path(session_id): Path<String>,
) -> (StatusCode, Json<Vec<EvaluationResult>>) {
    match state.results.list_for_session(&session_id).await {
        Ok(results) => (StatusCode::OK, Json(results)),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(vec![])),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::delete;
    use crate::store::mock::MockOutcomesStore;
    use tower::util::ServiceExt as _;

    fn make_app() -> Router {
        let store = MockOutcomesStore::new();
        let state = AppState {
            rubrics: RubricClient::new(store.clone()),
            results: ResultClient::new(store),
        };
        Router::new()
            .route(
                "/rubrics",
                post(handle_create_rubric::<MockOutcomesStore>)
                    .get(handle_list_rubrics::<MockOutcomesStore>),
            )
            .route(
                "/rubrics/{id}",
                get(handle_get_rubric::<MockOutcomesStore>)
                    .delete(handle_delete_rubric::<MockOutcomesStore>),
            )
            .route(
                "/evaluations/{session_id}",
                get(handle_list_results::<MockOutcomesStore>),
            )
            .route("/health", get(handle_health))
            .with_state(state)
    }

    fn rubric_body() -> serde_json::Value {
        serde_json::json!({
            "name": "Code Quality",
            "description": "Evaluates code review quality",
            "criteria": [
                {"name": "clarity", "description": "Is feedback clear?", "weight": 1.0}
            ]
        })
    }

    #[tokio::test]
    async fn health_returns_200() {
        let resp = make_app()
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn create_rubric_returns_201_with_id() {
        let resp = make_app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/rubrics")
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::to_vec(&rubric_body()).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let rubric: Rubric = serde_json::from_slice(&bytes).unwrap();
        assert!(!rubric.id.is_empty());
        assert_eq!(rubric.name, "Code Quality");
        assert_eq!(rubric.criteria.len(), 1);
    }

    #[tokio::test]
    async fn create_rubric_with_actor_type_filter_and_passing_score() {
        let body = serde_json::json!({
            "name": "PR Review",
            "description": "For PRs only",
            "actor_type_filter": "pr",
            "passing_score": 0.8,
            "criteria": [{"name": "q", "description": "d", "weight": 1.0}]
        });
        let resp = make_app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/rubrics")
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let rubric: Rubric = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(rubric.actor_type_filter.as_deref(), Some("pr"));
        assert!((rubric.passing_score - 0.8).abs() < 1e-5);
    }

    #[tokio::test]
    async fn list_rubrics_returns_all_created() {
        // Create two rubrics sequentially (oneshot consumes the app)
        let store = MockOutcomesStore::new();
        let state = AppState {
            rubrics: RubricClient::new(store.clone()),
            results: ResultClient::new(store.clone()),
        };
        let app = Router::new()
            .route(
                "/rubrics",
                post(handle_create_rubric::<MockOutcomesStore>)
                    .get(handle_list_rubrics::<MockOutcomesStore>),
            )
            .with_state(state);

        // Pre-populate via client
        let client = RubricClient::new(store);
        client.put(&Rubric::new("r1", "R1", "d", vec![])).await.unwrap();
        client.put(&Rubric::new("r2", "R2", "d", vec![])).await.unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/rubrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let rubrics: Vec<Rubric> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(rubrics.len(), 2);
    }

    #[tokio::test]
    async fn get_rubric_returns_404_when_not_found() {
        let resp = make_app()
            .oneshot(
                Request::builder()
                    .uri("/rubrics/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_rubric_returns_204() {
        let store = MockOutcomesStore::new();
        let state = AppState {
            rubrics: RubricClient::new(store.clone()),
            results: ResultClient::new(store.clone()),
        };
        let client = RubricClient::new(store);
        client.put(&Rubric::new("r1", "R1", "d", vec![])).await.unwrap();

        let app = Router::new()
            .route(
                "/rubrics/{id}",
                delete(handle_delete_rubric::<MockOutcomesStore>),
            )
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/rubrics/r1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn list_evaluations_returns_results_for_session() {
        use crate::types::{CriterionScore, EvaluationResult};

        let store = MockOutcomesStore::new();
        let state = AppState {
            rubrics: RubricClient::new(store.clone()),
            results: ResultClient::new(store.clone()),
        };

        let rubric = Rubric::new("r1", "R", "d", vec![Criterion {
            name: "q".into(), description: "d".into(), weight: 1.0
        }]);
        let result = EvaluationResult::compute(
            &rubric,
            "sess-1",
            "pr",
            "repo/1",
            vec![CriterionScore { criterion: "q".into(), score: 0.9, reasoning: "good".into() }],
            "overall ok",
        );
        ResultClient::new(store).put(&result).await.unwrap();

        let app = Router::new()
            .route(
                "/evaluations/{session_id}",
                get(handle_list_results::<MockOutcomesStore>),
            )
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/evaluations/sess-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let results: Vec<EvaluationResult> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].rubric_id, "r1");
        assert!(results[0].passed);
    }
}
