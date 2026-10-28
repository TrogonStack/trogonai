use std::convert::Infallible;
use std::io::Write;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use a2a_auth_callout::{BridgeMintRequest, BridgeMintResponse};
use a2a_bridge::{AuthCalloutClient, CallerHttpsAuth, StubAuthCalloutMint};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use bytes::Bytes;
use futures_util::StreamExt;
use tower::ServiceExt;
use trogon_nats::test_support::CoreTestServer;
use trogon_std::env::InMemoryEnv;
use trogon_std::log_capture::{CapturedLogs, LogLevel};

use super::*;

#[derive(Debug, thiserror::Error)]
enum TestError {
    #[error(transparent)]
    Bridge(#[from] BridgeError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Connect(#[from] async_nats::ConnectError),
    #[error(transparent)]
    Subscribe(#[from] async_nats::SubscribeError),
    #[error(transparent)]
    Unsubscribe(#[from] async_nats::UnsubscribeError),
    #[error(transparent)]
    Flush(#[from] async_nats::client::FlushError),
    #[error(transparent)]
    Publish(#[from] async_nats::PublishError),
    #[error(transparent)]
    Timeout(#[from] tokio::time::error::Elapsed),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Http(#[from] axum::http::Error),
    #[error(transparent)]
    Body(#[from] axum::Error),
    #[error(transparent)]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error(transparent)]
    Router(#[from] Infallible),
    #[error("request or reply subject missing")]
    MissingMessage,
}

#[derive(Clone, Default)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);

impl Write for LogCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn rpc_request() -> Result<Request<Body>, TestError> {
    Ok(Request::post("/")
        .header("authorization", "Bearer caller-fixture")
        .header("x-a2a-agent-id", "planner")
        .body(Body::from(
            r#"{"jsonrpc":"2.0","id":"bootstrap-request","method":"message/send","params":{}}"#,
        ))?)
}

#[test]
fn nats_server_list_normalizes_each_address_without_losing_tls() {
    assert_eq!(
        parse_nats_servers(" , localhost:4222, nats://second:4222, tls://third:4443, ,"),
        ["nats://localhost:4222", "nats://second:4222", "tls://third:4443"]
    );
    assert!(parse_nats_servers(" , , ").is_empty());
}

#[test]
fn timeout_input_accepts_trimmed_unsigned_values_and_rejects_invalid_values() {
    let env = InMemoryEnv::new();
    assert_eq!(parse_u64_env(&env, "TIMEOUT"), None);
    for (value, expected) in [(" 24 ", Some(24)), ("0", Some(0)), ("-1", None), ("overflow", None)] {
        env.set("TIMEOUT", value);
        assert_eq!(parse_u64_env(&env, "TIMEOUT"), expected);
    }
}

#[test]
fn bootstrap_prefix_uses_the_default_or_validated_supplied_namespace() -> Result<(), TestError> {
    let env = InMemoryEnv::new();
    assert_eq!(resolve_a2a_prefix(&env)?.as_str(), "a2a");
    env.set(ENV_A2A_PREFIX, "tenant");
    assert_eq!(resolve_a2a_prefix(&env)?.as_str(), "tenant");
    env.set(ENV_A2A_PREFIX, "invalid.*");
    assert!(matches!(resolve_a2a_prefix(&env), Err(BridgeError::NatsPublish(_))));
    Ok(())
}

#[tokio::test]
async fn run_rejects_invalid_configuration_before_serving() {
    let env = InMemoryEnv::new();
    env.set("BRIDGE_LISTEN_ADDR", "not-an-address");
    assert!(matches!(
        run(&env, std::future::pending()).await,
        Err(BootstrapError::BadListenAddr)
    ));
    env.set("BRIDGE_LISTEN_ADDR", "127.0.0.1:0");
    env.set("A2A_BRIDGE_TRANSPORT", "unsupported");
    assert!(
        matches!(run(&env, std::future::pending()).await, Err(BootstrapError::UnknownTransport(value)) if value == "unsupported")
    );
    env.set(ENV_A2A_PREFIX, "invalid.*");
    assert!(matches!(
        run(&env, std::future::pending()).await,
        Err(BootstrapError::Bridge(_))
    ));
    env.set(ENV_A2A_PREFIX, "tenant");
    env.set("A2A_BRIDGE_TRANSPORT", "nats");
    env.set("NATS_URL", " , ");
    assert!(matches!(
        run(&env, std::future::pending()).await,
        Err(BootstrapError::Bridge(_))
    ));
}

#[tokio::test]
async fn run_reports_an_occupied_listener() -> Result<(), TestError> {
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let env = InMemoryEnv::new();
    env.set("BRIDGE_LISTEN_ADDR", occupied.local_addr()?.to_string());
    assert!(
        matches!(run(&env, std::future::pending()).await, Err(BootstrapError::Listen(error)) if error.kind() == std::io::ErrorKind::AddrInUse)
    );
    Ok(())
}

#[tokio::test]
async fn run_completes_after_controlled_shutdown() {
    let env = InMemoryEnv::new();
    env.set("BRIDGE_LISTEN_ADDR", "127.0.0.1:0");
    assert!(run(&env, std::future::ready(())).await.is_ok());
}

#[tokio::test]
async fn stub_bootstrap_returns_an_explicit_authentication_failure() -> Result<(), TestError> {
    let env = InMemoryEnv::new();
    env.set("AUTH_CALLOUT_NATS_URL", "nats://auth:4222");
    let state = bootstrap_stub_transport(&env, "nats://unused:4222", resolve_a2a_prefix(&env)?);
    let response = gateway_router(state).oneshot(rpc_request()?).await?;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let payload: serde_json::Value = serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await?)?;
    assert_eq!(
        payload["error"],
        "auth callout mint failed: auth callout not configured for this deployment"
    );
    Ok(())
}

#[tokio::test]
async fn nats_bootstrap_reports_connection_failure() -> Result<(), TestError> {
    let env = InMemoryEnv::new();
    env.set("BRIDGE_CONNECT_TIMEOUT_SECS", "0");
    let result = bootstrap_nats_transport(&env, "invalid://endpoint", resolve_a2a_prefix(&env)?).await;
    assert!(matches!(result, Err(BridgeError::NatsPublish(_))));
    Ok(())
}

#[tokio::test]
async fn bootstrap_diagnostics_reach_the_log_facade_before_tracing_initialization() -> Result<(), TestError> {
    let Some(logs) = CapturedLogs::isolated() else {
        return Ok(());
    };
    let env = InMemoryEnv::new();
    env.set(ENV_A2A_PREFIX, "tenant");
    env.set("BRIDGE_CONNECT_TIMEOUT_SECS", "2");
    env.set("BRIDGE_AUTH_MINT_TIMEOUT_SECS", "3");
    env.set("BRIDGE_GATEWAY_RPC_TIMEOUT_SECS", "4");
    let _stub = bootstrap_stub_transport(&env, " nats://stub:4222 ", resolve_a2a_prefix(&env)?);
    let server = CoreTestServer::start().await;
    let _state = bootstrap_nats_transport(&env, server.address(), resolve_a2a_prefix(&env)?).await?;

    let records = logs.records();
    let stub = records
        .iter()
        .find(|record| record.message.contains("using stub transports"));
    assert!(stub.is_some_and(|record| record.level == LogLevel::Warn
        && record.message.contains("nats_url=nats://stub:4222")
        && record.message.contains("prefix=tenant")));
    let nats = records
        .iter()
        .find(|record| record.message.contains("a2a-bridge NATS transports wired"));
    assert!(nats.is_some_and(|record| record.level == LogLevel::Info
        && record.message.contains(server.address())
        && record.message.contains("connect_timeout_secs=2")
        && record.message.contains("mint_wire_secs=3")
        && record.message.contains("gateway_rpc_secs=4")
        && record.message.contains("prefix=tenant")));
    Ok(())
}

#[tokio::test]
async fn nats_bootstrap_wires_mint_subject_tenant_and_gateway_namespace() -> Result<(), TestError> {
    let logs = LogCapture::default();
    let writer = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .with_writer(move || writer.clone())
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let caller = CallerHttpsAuth::new("Bearer fixture");
    let jwt = StubAuthCalloutMint::fixture()?.mint(&caller).await?;
    let mint_reply = serde_json::to_vec(&BridgeMintResponse {
        user_jwt: jwt.as_str().to_owned(),
    })?;
    let expected = Bytes::from_static(br#"{"jsonrpc":"2.0","id":"bootstrap-request","result":{}}"#);
    for (configured_subject, expected_subject, account) in [
        (None, "a2a.bridge.auth.callout.request", None),
        (Some("  custom.mint  "), "custom.mint", Some("tenant-account")),
        (Some("   "), "a2a.bridge.auth.callout.request", Some("")),
    ] {
        let env = InMemoryEnv::new();
        env.set(ENV_A2A_PREFIX, "tenant");
        env.set("BRIDGE_CONNECT_TIMEOUT_SECS", "2");
        env.set("BRIDGE_AUTH_MINT_TIMEOUT_SECS", "2");
        env.set("BRIDGE_GATEWAY_RPC_TIMEOUT_SECS", "2");
        if configured_subject == Some("  custom.mint  ") {
            env.set("NATS_USER", "fixture");
            env.set("NATS_PASSWORD", "fixture");
        }
        if let Some(subject) = configured_subject {
            env.set("AUTH_CALLOUT_MINT_SUBJECT", subject);
        }
        if let Some(account) = account {
            env.set("BRIDGE_TENANT_ACCOUNT", account);
        }
        let mut mint = client.subscribe(expected_subject).await?;
        let mut gateway = client.subscribe("tenant.v1.gateway.planner.message.send").await?;
        client.flush().await?;
        let state = bootstrap_nats_transport(&env, server.address(), resolve_a2a_prefix(&env)?).await?;
        let handle_request = gateway_router(state).oneshot(rpc_request()?);
        let serve_wire = async {
            let request = tokio::time::timeout(Duration::from_secs(5), mint.next())
                .await?
                .ok_or(TestError::MissingMessage)?;
            let envelope: BridgeMintRequest = serde_json::from_slice(&request.payload)?;
            assert_eq!(envelope.user_jwt.as_deref(), Some("caller-fixture"));
            assert_eq!(envelope.account.as_deref(), account.filter(|value| !value.is_empty()));
            client
                .publish(
                    request.reply.ok_or(TestError::MissingMessage)?,
                    mint_reply.clone().into(),
                )
                .await?;
            let request = tokio::time::timeout(Duration::from_secs(5), gateway.next())
                .await?
                .ok_or(TestError::MissingMessage)?;
            let payload: serde_json::Value = serde_json::from_slice(&request.payload)?;
            assert_eq!(payload["id"], "bootstrap-request");
            assert_eq!(
                request
                    .headers
                    .and_then(|headers| headers.get("Trogon-Req-Id").map(ToString::to_string))
                    .as_deref(),
                Some("bootstrap-request")
            );
            client
                .publish(request.reply.ok_or(TestError::MissingMessage)?, expected.clone())
                .await?;
            Ok::<(), TestError>(())
        };
        let (response, wire) = tokio::join!(handle_request, serve_wire);
        wire?;
        let response = response?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(to_bytes(response.into_body(), usize::MAX).await?, expected);
        mint.unsubscribe().await?;
        gateway.unsubscribe().await?;
    }
    let output = String::from_utf8(logs.0.lock().unwrap_or_else(PoisonError::into_inner).clone())?;
    assert!(output.contains("a2a-bridge NATS transports wired"));
    assert!(output.contains("prefix=tenant"));
    assert!(output.contains("connect_timeout_secs=2"));
    assert!(output.contains("mint_wire_secs=2"));
    assert!(output.contains("gateway_rpc_secs=2"));
    Ok(())
}
