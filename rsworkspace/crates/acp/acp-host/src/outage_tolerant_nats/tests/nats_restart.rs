//! A real NATS server restarts while the child asks the client for
//! permission, and the client bridge comes back after the host does: the
//! request waits out the outage and the client answers it exactly once.

use super::*;
use crate::constants::DEFAULT_CLIENT_RETRY_WINDOW;
use crate::session_routed_client::SessionRoutedClient;
use acp_nats::{AcpPrefix, Bridge, ClientHandler, Config, NatsConfig};
use agent_client_protocol::v1::schema::{
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SessionNotification, ToolCallUpdate, ToolCallUpdateFields,
};
use agent_client_protocol::{ErrorCode, Result};
use std::sync::atomic::AtomicUsize;
use trogon_nats::test_support::RestartableTestServer;
use trogon_std::time::SystemClock;

/// How long the server stays down while the request is outstanding.
const OUTAGE: Duration = Duration::from_millis(300);
/// How long the client bridge stays away after the host has reconnected.
const BRIDGE_LAG: Duration = Duration::from_secs(1);

async fn connect(server: &RestartableTestServer) -> async_nats::Client {
    async_nats::ConnectOptions::new()
        .reconnect_delay_callback(|_| Duration::from_millis(100))
        .connect(server.address())
        .await
        .expect("connect to NATS")
}

/// The client at the far end of the bridge: refuses every permission request
/// and counts how many it received.
#[derive(Default)]
struct CountingClient {
    permission_requests: AtomicUsize,
}

#[async_trait::async_trait]
impl ClientHandler for CountingClient {
    async fn request_permission(&self, _args: RequestPermissionRequest) -> Result<RequestPermissionResponse> {
        self.permission_requests.fetch_add(1, Ordering::SeqCst);
        Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
    }

    async fn session_notification(&self, _args: SessionNotification) -> Result<()> {
        Ok(())
    }
}

/// Starts the real client bridge on its own connection.
async fn start_bridge(server: &RestartableTestServer, prefix: &AcpPrefix, client: Arc<CountingClient>) {
    let nats = connect(server).await;
    let bridge = Arc::new(Bridge::new(
        nats.clone(),
        (),
        SystemClock,
        &opentelemetry::global::meter("acp-host-nats-restart"),
        Config::new(prefix.clone(), NatsConfig::from_url(server.address())),
    ));
    tokio::spawn(acp_nats::client::run(nats, client, bridge));
}

fn permission_request() -> RequestPermissionRequest {
    RequestPermissionRequest::new(
        "s1",
        ToolCallUpdate::new("call-1", ToolCallUpdateFields::new()),
        vec![PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce)],
    )
}

/// Restarts the server while the child's permission request is outstanding,
/// with the client bridge back [`BRIDGE_LAG`] after the host, and returns what
/// the child heard and how many requests the client received.
async fn ask_across_a_restart(window: Duration) -> (Result<RequestPermissionOutcome>, usize) {
    let server = RestartableTestServer::start().await;
    let prefix = AcpPrefix::new("acp").expect("prefix");
    let host = connect(&server).await;
    let routed = SessionRoutedClient::new(
        OutageTolerantNats::new(host.clone(), window),
        prefix.clone(),
        Duration::from_secs(30),
    );

    server.stop().await;
    let asked = tokio::spawn(async move { routed.request_permission(permission_request()).await });
    tokio::time::sleep(OUTAGE).await;
    server.restart().await;
    until_connected(&host, Instant::now() + Duration::from_secs(10)).await;
    assert!(host.is_connected(), "the host reconnects");

    tokio::time::sleep(BRIDGE_LAG).await;
    let client = Arc::new(CountingClient::default());
    start_bridge(&server, &prefix, client.clone()).await;

    let answer = asked.await.expect("request task").map(|response| response.outcome);
    (answer, client.permission_requests.load(Ordering::SeqCst))
}

#[tokio::test]
async fn a_permission_request_waits_out_a_nats_restart_and_reaches_the_client_once() {
    let (answer, received) = ask_across_a_restart(DEFAULT_CLIENT_RETRY_WINDOW).await;

    assert_eq!(
        answer.expect("the client answered"),
        RequestPermissionOutcome::Cancelled
    );
    assert_eq!(received, 1, "the client received the request exactly once");
}

#[tokio::test]
async fn without_the_window_the_same_restart_fails_the_request_with_no_responders() {
    let (answer, received) = ask_across_a_restart(Duration::ZERO).await;

    let error = answer.expect_err("nobody was subscribed when the host reconnected");
    assert_eq!(error.code, ErrorCode::INTERNAL_ERROR);
    assert!(error.message.contains("no responders"), "{}", error.message);
    assert_eq!(received, 0);
}
