use axum::extract::ws::{WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use tokio::sync::{mpsc, watch};
use tracing::{error, warn};

pub struct ConnectionRequest {
    pub socket: WebSocket,
    pub shutdown_rx: watch::Receiver<bool>,
    /// When `Some`, overrides the connection thread's default ACP config.
    pub config_override: Option<acp_nats::Config>,
}

#[derive(Clone)]
pub struct UpgradeState {
    pub conn_tx: mpsc::UnboundedSender<ConnectionRequest>,
    pub shutdown_tx: watch::Sender<bool>,
}

/// Plain WebSocket upgrade handler — legacy `/ws` route; routes without an agent
/// type and uses the thread-default ACP config.  New clients should use `/ws/:agent_type`.
///
/// Only used by integration tests; `#[allow(dead_code)]` prevents the lint from firing
/// when the function is not reachable from the binary's `main`.
#[allow(dead_code)]
pub async fn handle(ws: WebSocketUpgrade, State(state): State<UpgradeState>) -> Response {
    let shutdown_rx = state.shutdown_tx.subscribe();
    ws.on_upgrade(move |socket| async move {
        if state
            .conn_tx
            .send(ConnectionRequest {
                socket,
                shutdown_rx,
                config_override: None,
            })
            .is_err()
        {
            error!("Connection thread is gone; dropping WebSocket");
        }
    })
}

/// Resolve the ACP `Config` for `agent_type` by consulting the registry.
///
/// Generic over the registry store so unit tests can inject a `MockRegistryStore`
/// without a live NATS connection.
pub(crate) async fn config_for_agent<S: trogon_registry::RegistryStore>(
    agent_type: &str,
    registry: &trogon_registry::Registry<S>,
    base_config: &acp_nats::Config,
) -> Result<acp_nats::Config, (StatusCode, String)> {
    match registry.get(agent_type).await {
        Ok(Some(cap)) => {
            let prefix_str = cap
                .metadata
                .get("acp_prefix")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| {
                    warn!(agent_type, "registry entry missing acp_prefix metadata, using nats_subject");
                    cap.nats_subject.trim_end_matches(".agent.>")
                });
            acp_nats::AcpPrefix::new(prefix_str)
                .map(|prefix| base_config.for_prefix(prefix))
                .map_err(|e| {
                    error!(agent_type, error = %e, "invalid acp_prefix in registry");
                    (StatusCode::INTERNAL_SERVER_ERROR, "invalid agent prefix in registry".to_string())
                })
        }
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            format!("agent type '{agent_type}' is not registered"),
        )),
        Err(e) => {
            error!(agent_type, error = %e, "registry lookup failed");
            Err((StatusCode::SERVICE_UNAVAILABLE, "registry temporarily unavailable".to_string()))
        }
    }
}

/// Handler for `/ws/:agent_type` — looks up the agent type in the registry and
/// creates a connection with the agent's ACP prefix.
#[cfg_attr(coverage, coverage(off))]
pub async fn handle_with_agent_type<S: trogon_registry::RegistryStore + 'static>(
    Path(agent_type): Path<String>,
    State(state): State<UpgradeState>,
    axum::extract::Extension(registry_ext): axum::extract::Extension<
        std::sync::Arc<RegistryExtension<S>>,
    >,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let config_override =
        match config_for_agent(&agent_type, &registry_ext.registry, &registry_ext.base_config)
            .await
        {
            Ok(c) => c,
            Err((status, msg)) => return (status, msg).into_response(),
        };

    let shutdown_rx = state.shutdown_tx.subscribe();
    ws.on_upgrade(move |socket| async move {
        if state
            .conn_tx
            .send(ConnectionRequest {
                socket,
                shutdown_rx,
                config_override: Some(config_override),
            })
            .is_err()
        {
            error!("Connection thread is gone; dropping WebSocket");
        }
    })
}

/// Shared registry state injected via axum Extension for the `/ws/:agent_type` route.
///
/// Generic over the registry store so tests can inject a [`MockRegistryStore`][trogon_registry::MockRegistryStore]
/// without a live NATS connection. Production builds use the default `S = `[`KvStore`][trogon_registry::KvStore].
#[derive(Clone)]
pub struct RegistryExtension<S: trogon_registry::RegistryStore = trogon_registry::KvStore> {
    pub registry: trogon_registry::Registry<S>,
    pub base_config: acp_nats::Config,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio_tungstenite::connect_async;
    use trogon_registry::{AgentCapability, MockRegistryStore, Registry};

    // ── FailingRegistryStore — injects a get() error for 503 branch tests ────

    #[derive(Clone)]
    struct FailingRegistryStore;

    #[derive(Debug)]
    struct StoreUnavailable;
    impl std::fmt::Display for StoreUnavailable {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "store unavailable")
        }
    }
    impl std::error::Error for StoreUnavailable {}

    impl trogon_registry::RegistryStore for FailingRegistryStore {
        type PutError = StoreUnavailable;
        type GetError = StoreUnavailable;
        type DeleteError = StoreUnavailable;
        type KeysError = StoreUnavailable;
        async fn put(&self, _: &str, _: bytes::Bytes) -> Result<u64, Self::PutError> { Ok(0) }
        async fn get(&self, _: &str) -> Result<Option<bytes::Bytes>, Self::GetError> {
            Err(StoreUnavailable)
        }
        async fn delete(&self, _: &str) -> Result<(), Self::DeleteError> { Ok(()) }
        async fn keys(&self) -> Result<Vec<String>, Self::KeysError> { Ok(vec![]) }
    }

    fn base_config() -> acp_nats::Config {
        acp_nats::Config::new(
            acp_nats::AcpPrefix::new("acp").unwrap(),
            acp_nats::NatsConfig {
                servers: vec!["localhost:4222".to_string()],
                auth: acp_nats::NatsAuth::None,
            },
        )
    }

    fn mock_registry() -> Registry<MockRegistryStore> {
        Registry::new(MockRegistryStore::new())
    }

    #[tokio::test]
    async fn config_for_agent_registered_returns_prefixed_config() {
        let reg = mock_registry();
        let cap = AgentCapability {
            agent_type: "claude".to_string(),
            capabilities: vec!["chat".to_string()],
            nats_subject: "acp.claude.agent.>".to_string(),
            current_load: 0,
            metadata: serde_json::json!({ "acp_prefix": "acp.claude" }),
        };
        reg.register(&cap).await.unwrap();
        let result = config_for_agent("claude", &reg, &base_config()).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().acp_prefix(), "acp.claude");
    }

    #[tokio::test]
    async fn config_for_agent_unregistered_returns_not_found() {
        let reg = mock_registry();
        let (status, _) = config_for_agent("unknown", &reg, &base_config()).await.err().unwrap();
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn config_for_agent_falls_back_to_nats_subject_without_metadata() {
        let reg = mock_registry();
        let cap = AgentCapability {
            agent_type: "xai".to_string(),
            capabilities: vec!["chat".to_string()],
            nats_subject: "acp.xai.agent.>".to_string(),
            current_load: 0,
            metadata: serde_json::Value::Null,
        };
        reg.register(&cap).await.unwrap();
        let result = config_for_agent("xai", &reg, &base_config()).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().acp_prefix(), "acp.xai");
    }

    #[tokio::test]
    async fn config_for_agent_invalid_prefix_returns_internal_error() {
        let reg = mock_registry();
        let cap = AgentCapability {
            agent_type: "bad".to_string(),
            capabilities: vec!["chat".to_string()],
            nats_subject: "acp.bad.agent.>".to_string(),
            current_load: 0,
            metadata: serde_json::json!({ "acp_prefix": "invalid prefix!" }),
        };
        reg.register(&cap).await.unwrap();
        let (status, _) = config_for_agent("bad", &reg, &base_config()).await.err().unwrap();
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn config_for_agent_store_error_returns_service_unavailable() {
        let reg = Registry::new(FailingRegistryStore);
        let (status, _) = config_for_agent("any", &reg, &base_config()).await.err().unwrap();
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn handle_with_agent_type_sends_config_override_for_registered_agent() {
        let reg = mock_registry();
        let cap = AgentCapability {
            agent_type: "claude".to_string(),
            capabilities: vec!["chat".to_string()],
            nats_subject: "acp.claude.agent.>".to_string(),
            current_load: 0,
            metadata: serde_json::json!({ "acp_prefix": "acp.claude" }),
        };
        reg.register(&cap).await.unwrap();

        let registry_ext = std::sync::Arc::new(RegistryExtension {
            registry: reg,
            base_config: base_config(),
        });

        let (shutdown_tx, _) = watch::channel(false);
        let (conn_tx, mut conn_rx) = tokio::sync::mpsc::unbounded_channel::<ConnectionRequest>();
        let state = UpgradeState { conn_tx, shutdown_tx: shutdown_tx.clone() };

        let app = axum::Router::new()
            .route(
                "/ws/{agent_type}",
                axum::routing::get(handle_with_agent_type::<MockRegistryStore>),
            )
            .layer(axum::extract::Extension(registry_ext))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let url = format!("ws://{}/ws/claude", addr);
        let (_ws, _) = connect_async(&url).await.unwrap();

        let req = tokio::time::timeout(Duration::from_secs(2), conn_rx.recv())
            .await
            .expect("timeout waiting for ConnectionRequest")
            .expect("channel closed");

        let config = req.config_override.expect("ConnectionRequest must have config_override");
        assert_eq!(config.acp_prefix(), "acp.claude");
    }

    #[tokio::test]
    async fn handle_with_agent_type_returns_404_for_unregistered_agent() {
        let reg = mock_registry(); // empty — no agents registered

        let registry_ext = std::sync::Arc::new(RegistryExtension {
            registry: reg,
            base_config: base_config(),
        });

        let (shutdown_tx, _) = watch::channel(false);
        let (conn_tx, _conn_rx) = tokio::sync::mpsc::unbounded_channel::<ConnectionRequest>();
        let state = UpgradeState { conn_tx, shutdown_tx };

        let app = axum::Router::new()
            .route(
                "/ws/{agent_type}",
                axum::routing::get(handle_with_agent_type::<MockRegistryStore>),
            )
            .layer(axum::extract::Extension(registry_ext))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let url = format!("ws://{}/ws/ghost", addr);
        let result = connect_async(&url).await;
        // The server returns 404; tungstenite surfaces it as a non-101 response error.
        match result {
            Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
                assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
            }
            Ok(_) => panic!("expected WebSocket upgrade to be rejected with 404"),
            Err(e) => panic!("unexpected error: {e}"),
        }
    }

    #[tokio::test]
    async fn handle_sends_connection_request_through_channel() {
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        let (conn_tx, mut conn_rx) = mpsc::unbounded_channel::<ConnectionRequest>();

        let state = UpgradeState {
            conn_tx,
            shutdown_tx: shutdown_tx.clone(),
        };

        let app = axum::Router::new()
            .route("/ws", axum::routing::get(handle))
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let url = format!("ws://{}/ws", addr);
        let (_ws, _) = connect_async(&url).await.unwrap();

        let req = tokio::time::timeout(Duration::from_secs(2), conn_rx.recv())
            .await
            .expect("timeout waiting for ConnectionRequest")
            .expect("channel closed");

        assert!(!*req.shutdown_rx.borrow());
    }

    #[tokio::test]
    async fn handle_logs_error_when_conn_rx_dropped() {
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        let (conn_tx, conn_rx) = mpsc::unbounded_channel::<ConnectionRequest>();

        let state = UpgradeState {
            conn_tx,
            shutdown_tx: shutdown_tx.clone(),
        };

        let app = axum::Router::new()
            .route("/ws", axum::routing::get(handle))
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        drop(conn_rx);

        let url = format!("ws://{}/ws", addr);
        let (_ws, _) = connect_async(&url).await.unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
