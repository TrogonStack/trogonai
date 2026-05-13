use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use serde::Deserialize;
use tokio::sync::Mutex;
use tracing::info;
use trogon_registry::RegistryStore;

use crate::{
    caller::AgentCaller,
    engine::OrchestratorEngine,
    provider::OrchestratorProvider,
    types::{OrchestrationResult, OrchestratorError},
};

// ── AppState ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState<P: OrchestratorProvider, C: AgentCaller, S: RegistryStore> {
    engine: Arc<OrchestratorEngine<P, C, S>>,
    results: Arc<Mutex<Vec<OrchestrationResult>>>,
}

// ── Request types ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct OrchestrationRequest {
    task: String,
}

// ── Public entry point ────────────────────────────────────────────────────────

#[cfg(not(coverage))]
pub async fn serve<P: OrchestratorProvider, C: AgentCaller, S: RegistryStore>(
    port: u16,
    engine: OrchestratorEngine<P, C, S>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    serve_impl(port, engine).await
}

async fn serve_impl<P: OrchestratorProvider, C: AgentCaller, S: RegistryStore>(
    port: u16,
    engine: OrchestratorEngine<P, C, S>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = AppState {
        engine: Arc::new(engine),
        results: Arc::new(Mutex::new(Vec::new())),
    };

    let app = Router::new()
        .route("/orchestrate", post(handle_orchestrate::<P, C, S>))
        .route("/results", get(handle_list_results::<P, C, S>))
        .route("/health", get(handle_health))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!(addr = %addr, "Orchestrator management API listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("Orchestrator management API shut down");
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

async fn handle_orchestrate<P: OrchestratorProvider, C: AgentCaller, S: RegistryStore>(
    State(state): State<AppState<P, C, S>>,
    Json(body): Json<OrchestrationRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    match state.engine.orchestrate(&body.task).await {
        Ok(result) => {
            let json = serde_json::to_value(&result).unwrap_or(serde_json::Value::Null);
            state.results.lock().await.push(result);
            (StatusCode::OK, Json(json))
        }
        Err(OrchestratorError::Planning(e)) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"error": format!("planning failed: {e}")})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

async fn handle_list_results<P: OrchestratorProvider, C: AgentCaller, S: RegistryStore>(
    State(state): State<AppState<P, C, S>>,
) -> (StatusCode, Json<Vec<OrchestrationResult>>) {
    let results = state.results.lock().await.clone();
    (StatusCode::OK, Json(results))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::util::ServiceExt as _;
    use trogon_registry::{MockRegistryStore, Registry};

    use crate::{
        caller::mock::MockAgentCaller,
        types::{SubTask, TaskPlan},
    };

    use std::future::Future;

    #[derive(Clone)]
    struct NoOpProvider;

    impl OrchestratorProvider for NoOpProvider {
        fn plan_task<'a>(
            &'a self,
            task: &'a str,
            _capabilities: &'a [trogon_registry::AgentCapability],
        ) -> impl Future<Output = Result<TaskPlan, OrchestratorError>> + Send + 'a {
            let task = task.to_string();
            async move {
                Ok(TaskPlan {
                    subtasks: vec![SubTask {
                        id: "1".into(),
                        capability: "mock".into(),
                        description: task,
                        payload: b"{}".to_vec(),
                    }],
                    reasoning: "test plan".into(),
                })
            }
        }

        fn synthesize_results<'a>(
            &'a self,
            _task: &'a str,
            _plan: &'a TaskPlan,
            _results: &'a [crate::types::SubTaskResult],
        ) -> impl Future<Output = Result<String, OrchestratorError>> + Send + 'a {
            async move { Ok("synthesized answer".to_string()) }
        }
    }

    fn make_app() -> Router {
        let store = MockRegistryStore::new();
        let registry = Registry::new(store);
        let caller = MockAgentCaller::returning(b"agent response".to_vec());
        let engine = OrchestratorEngine::new(NoOpProvider, caller, registry);
        let state: AppState<NoOpProvider, MockAgentCaller, MockRegistryStore> = AppState {
            engine: Arc::new(engine),
            results: Arc::new(Mutex::new(Vec::new())),
        };
        Router::new()
            .route(
                "/orchestrate",
                post(handle_orchestrate::<NoOpProvider, MockAgentCaller, MockRegistryStore>),
            )
            .route(
                "/results",
                get(handle_list_results::<NoOpProvider, MockAgentCaller, MockRegistryStore>),
            )
            .route("/health", get(handle_health))
            .with_state(state)
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
    async fn orchestrate_returns_200_with_result() {
        let body = serde_json::json!({"task": "Review PR #1"});
        let resp = make_app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/orchestrate")
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let result: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result["synthesis"].as_str().unwrap(), "synthesized answer");
        assert_eq!(result["task"].as_str().unwrap(), "Review PR #1");
    }

    #[tokio::test]
    async fn list_results_returns_all_previous_results() {
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let store = MockRegistryStore::new();
        let registry = Registry::new(store);
        let caller = MockAgentCaller::returning(b"ok".to_vec());
        let engine =
            OrchestratorEngine::new(NoOpProvider, caller, registry);
        let results_store: Arc<Mutex<Vec<OrchestrationResult>>> =
            Arc::new(Mutex::new(Vec::new()));
        let state: AppState<NoOpProvider, MockAgentCaller, MockRegistryStore> = AppState {
            engine: Arc::new(engine),
            results: Arc::clone(&results_store),
        };

        let app = Router::new()
            .route(
                "/orchestrate",
                post(handle_orchestrate::<NoOpProvider, MockAgentCaller, MockRegistryStore>),
            )
            .route(
                "/results",
                get(handle_list_results::<NoOpProvider, MockAgentCaller, MockRegistryStore>),
            )
            .with_state(state);

        // Submit one task
        let body = serde_json::json!({"task": "Task A"});
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/orchestrate")
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        // List results
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/results")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let results: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn orchestrate_returns_422_when_planning_fails() {
        // Provider that always fails at planning
        #[derive(Clone)]
        struct FailingProvider;
        impl OrchestratorProvider for FailingProvider {
            fn plan_task<'a>(
                &'a self,
                _task: &'a str,
                _capabilities: &'a [trogon_registry::AgentCapability],
            ) -> impl Future<Output = Result<TaskPlan, OrchestratorError>> + Send + 'a {
                async { Err(OrchestratorError::Planning("LLM unavailable".into())) }
            }
            fn synthesize_results<'a>(
                &'a self,
                _task: &'a str,
                _plan: &'a TaskPlan,
                _results: &'a [crate::types::SubTaskResult],
            ) -> impl Future<Output = Result<String, OrchestratorError>> + Send + 'a {
                async { Ok(String::new()) }
            }
        }

        let store = MockRegistryStore::new();
        let registry = Registry::new(store);
        let caller = MockAgentCaller::returning(b"".to_vec());
        let engine = OrchestratorEngine::new(FailingProvider, caller, registry);
        let state: AppState<FailingProvider, MockAgentCaller, MockRegistryStore> = AppState {
            engine: Arc::new(engine),
            results: Arc::new(Mutex::new(Vec::new())),
        };
        let app = Router::new()
            .route(
                "/orchestrate",
                post(handle_orchestrate::<FailingProvider, MockAgentCaller, MockRegistryStore>),
            )
            .with_state(state);

        let body = serde_json::json!({"task": "do something"});
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/orchestrate")
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
