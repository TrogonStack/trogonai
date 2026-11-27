use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use trogon_sandbox_auth::{ApiKeyRecord, BearerToken, InMemoryApiKeyStore};
use trogon_sandbox_contract::{ServiceAccountId, TenantId};
use trogon_sandbox_ws_gateway::{API_KEY_HEADER, AppState, InMemorySessionStore, SandboxBackend, router};

pub type TestResult<T = ()> = Result<T, Box<dyn Error>>;

pub type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// An API key made at run time so no key literal sits in the tests.
pub fn api_key(label: &str) -> String {
    format!("key-{label}-{}", std::process::id())
}

/// The shared state of a gateway that knows `api_key` as `tenant`.
pub async fn test_state(api_key: &str, tenant: &str, backend: Arc<dyn SandboxBackend>) -> TestResult<AppState> {
    let api_keys = InMemoryApiKeyStore::new();
    let record = ApiKeyRecord::new(
        TenantId::new(tenant)?,
        ServiceAccountId::new("service-account"),
        "test key",
        SystemTime::now(),
    );
    api_keys.insert(BearerToken::new(api_key)?.hash(), record).await;
    Ok(AppState {
        sessions: Arc::new(InMemorySessionStore::new()),
        api_keys: Arc::new(api_keys),
        backend,
        session_ttl: Duration::from_secs(3600),
        preview: None,
    })
}

/// Serves `state` on a free local port.
pub async fn serve(state: AppState) -> TestResult<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let app = router(state, vec![]);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(addr)
}

/// Mints a session and returns its token.
pub async fn session_token(addr: SocketAddr, api_key: &str) -> TestResult<String> {
    let session: Value = reqwest::Client::new()
        .post(format!("http://{addr}/v1/sessions"))
        .header(API_KEY_HEADER, api_key)
        .send()
        .await?
        .json()
        .await?;
    Ok(session["token"].as_str().ok_or("no session token")?.to_string())
}

/// Opens a websocket for `token`.
pub async fn connect_with_token(addr: SocketAddr, token: &str) -> TestResult<Socket> {
    let mut request = format!("ws://{addr}/v1/ws").into_client_request()?;
    request
        .headers_mut()
        .insert("sec-websocket-protocol", format!("bearer.{token}").parse()?);
    let (socket, _) = tokio_tungstenite::connect_async(request).await?;
    Ok(socket)
}

/// Starts a gateway over `backend` and opens a socket as `api_key`.
pub async fn open_socket(backend: Arc<dyn SandboxBackend>, api_key: &str, tenant: &str) -> TestResult<Socket> {
    let addr = serve(test_state(api_key, tenant, backend).await?).await?;
    let token = session_token(addr, api_key).await?;
    connect_with_token(addr, &token).await
}

pub async fn send(socket: &mut Socket, id: &str, tool: &str, payload: Value) -> TestResult {
    let request = json!({"kind": "request", "id": id, "tool": tool, "payload": payload});
    socket.send(Message::Text(request.to_string().into())).await?;
    Ok(())
}

async fn read_frame(socket: &mut Socket, kind: &str) -> TestResult<Value> {
    loop {
        let message = tokio::time::timeout(WAIT, socket.next())
            .await?
            .ok_or("socket closed")??;
        if let Message::Text(text) = message {
            let frame: Value = serde_json::from_str(&text)?;
            if frame["kind"] == kind {
                return Ok(frame);
            }
        }
    }
}

pub async fn read_response(socket: &mut Socket) -> TestResult<Value> {
    read_frame(socket, "response").await
}

pub async fn read_event(socket: &mut Socket) -> TestResult<Value> {
    read_frame(socket, "event").await
}

/// Sends one request and returns its response frame.
pub async fn call(socket: &mut Socket, tool: &str, payload: Value) -> TestResult<Value> {
    send(socket, "req", tool, payload).await?;
    read_response(socket).await
}

/// The result of a call that must have succeeded.
pub async fn call_ok(socket: &mut Socket, tool: &str, payload: Value) -> TestResult<Value> {
    let frame = call(socket, tool, payload).await?;
    if frame["error"].is_null() {
        Ok(frame["result"].clone())
    } else {
        Err(format!("{tool} failed: {frame}").into())
    }
}

/// The error code of a call that must have failed.
pub async fn call_err(socket: &mut Socket, tool: &str, payload: Value) -> TestResult<String> {
    let frame = call(socket, tool, payload).await?;
    Ok(frame["error"]["code"]
        .as_str()
        .ok_or_else(|| format!("{tool} did not fail: {frame}"))?
        .to_string())
}

pub const WAIT: Duration = Duration::from_secs(10);

/// The next text frame, whatever its kind.
pub async fn read_next(socket: &mut Socket) -> TestResult<Value> {
    loop {
        let message = tokio::time::timeout(WAIT, socket.next())
            .await?
            .ok_or("socket closed")??;
        if let Message::Text(text) = message {
            return Ok(serde_json::from_str(&text)?);
        }
    }
}

/// Releases a paused mock command when dropped, so a failing test cannot leave the gateway waiting.
pub struct Release(pub Arc<tokio::sync::Notify>);

impl Drop for Release {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

/// Serves `app` on a free local port.
pub async fn serve_router(app: axum::Router) -> TestResult<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(addr)
}

/// An address nothing listens on.
pub async fn closed_address() -> TestResult<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    Ok(listener.local_addr()?)
}

/// Preview signing keys as the `kid=hexkey` specification, built at run time.
pub fn key_spec(kid: &str, byte: &str) -> String {
    format!("{kid}={}", byte.repeat(32))
}

/// A session store whose every operation fails.
pub struct FailingSessionStore;

#[derive(Debug, thiserror::Error)]
#[error("store unavailable")]
struct UnavailableError;

#[async_trait::async_trait]
impl trogon_sandbox_ws_gateway::SessionStore for FailingSessionStore {
    async fn put(
        &self,
        _token_hash: &str,
        _record: trogon_sandbox_ws_gateway::SessionRecord,
    ) -> Result<(), trogon_sandbox_ws_gateway::SessionStoreError> {
        Err(trogon_sandbox_ws_gateway::SessionStoreError::Backend(Box::new(
            UnavailableError,
        )))
    }

    async fn lookup(
        &self,
        _token_hash: &str,
    ) -> Result<Option<trogon_sandbox_ws_gateway::SessionRecord>, trogon_sandbox_ws_gateway::SessionStoreError> {
        Err(trogon_sandbox_ws_gateway::SessionStoreError::Backend(Box::new(
            UnavailableError,
        )))
    }
}

/// A complete lease request for `tenant`.
pub fn lease_request(tenant: &str) -> Value {
    json!({
        "tenant": tenant,
        "provider_pref": [],
        "ttl_seconds": 60,
        "idempotency_key": "lease-key-1",
        "spec": {
            "image": {"kind": "registry", "reference": {"registry": "ghcr.io", "repository": "acme/app"}},
            "workspace": {"empty": {}},
            "cpu_seconds": 10,
            "memory_mib": 512,
            "env": {},
            "privileged": false,
            "boot_mode": "entrypoint_default",
        },
    })
}
