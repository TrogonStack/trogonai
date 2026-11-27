//! A client extension call must reach the agent under the exact name the client used.
//!
//! The path is client JSON-RPC → bridge boundary → NATS → agent-side connection → agent.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use acp_nats::boundary::{BoundaryExit, connect_agent_boundary};
use acp_nats::nats::global;
use acp_nats::{AcpPrefix, AgentHandler, Bridge, Config, NatsConfig, NatsJetStreamClient};
use acp_nats_agent::AgentSideNatsConnection;
use agent_client_protocol::Result;
use agent_client_protocol::v1::schema::{
    AuthenticateRequest, AuthenticateResponse, CancelNotification, ExtNotification, ExtRequest, ExtResponse,
    InitializeRequest, InitializeResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    StopReason,
};
use jsonrpc_nats::RequestId;
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader, Lines};
use tokio::sync::Notify;
use tokio::time::timeout;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_std::time::SystemClock;

const EXT_METHOD: &str = "_vendor/method";
const WAIT: Duration = Duration::from_secs(10);

/// Notification names the agent saw. Requests echo their name in the reply instead.
#[derive(Default)]
struct Notifications {
    names: Mutex<Vec<String>>,
    arrived: Notify,
}

struct RecordingAgent(Arc<Notifications>);

#[async_trait::async_trait]
impl AgentHandler for RecordingAgent {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        Ok(InitializeResponse::new(args.protocol_version))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, _args: NewSessionRequest) -> Result<NewSessionResponse> {
        Ok(NewSessionResponse::new("session-1"))
    }

    async fn prompt(&self, _args: PromptRequest) -> Result<PromptResponse> {
        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn cancel(&self, _args: CancelNotification) -> Result<()> {
        Ok(())
    }

    async fn ext_method(&self, args: ExtRequest) -> Result<ExtResponse> {
        let params: Value = serde_json::from_str(args.params.get()).unwrap();
        let reply = json!({ "method": args.method.as_ref(), "params": params });
        Ok(ExtResponse::new(
            serde_json::value::to_raw_value(&reply).unwrap().into(),
        ))
    }

    async fn ext_notification(&self, args: ExtNotification) -> Result<()> {
        self.0.names.lock().unwrap().push(args.method.to_string());
        self.0.arrived.notify_one();
        Ok(())
    }
}

async fn send_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &Value) {
    let mut bytes = serde_json::to_vec(frame).unwrap();
    bytes.push(b'\n');
    writer.write_all(&bytes).await.unwrap();
}

async fn read_frame<R: AsyncBufRead + Unpin>(lines: &mut Lines<R>) -> Value {
    let line = timeout(WAIT, lines.next_line()).await.unwrap().unwrap().unwrap();
    serde_json::from_str(&line).unwrap()
}

/// Waits until the agent-side connection answers on NATS, so the client's
/// first request does not race the agent's subscription.
async fn wait_for_agent(nats: &async_nats::Client, prefix: &AcpPrefix) {
    let subject = global::InitializeSubject::new(prefix).to_string();
    let request = InitializeRequest::new(agent_client_protocol::schema::ProtocolVersion::V1);
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let encoded = acp_nats::wire::encode_request("initialize", RequestId::Number(0), &request).unwrap();
        let answer = nats
            .request_with_headers(subject.clone(), encoded.headers, encoded.body)
            .await;
        if answer.is_ok() {
            return;
        }
        assert!(tokio::time::Instant::now() < deadline, "agent never answered on NATS");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn extension_names_reach_the_agent_unchanged_through_nats() {
    let server = JetStreamTestServer::start().await;
    let prefix = AcpPrefix::new("ext_names").unwrap();
    let notifications = Arc::new(Notifications::default());

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let agent_nats = server.client().await;
            let (_conn, serve) = AgentSideNatsConnection::new(
                RecordingAgent(notifications.clone()),
                agent_nats,
                prefix.clone(),
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );
            tokio::task::spawn_local(serve);

            let bridge_nats = server.client().await;
            wait_for_agent(&bridge_nats, &prefix).await;
            let bridge = Arc::new(Bridge::new(
                bridge_nats,
                NatsJetStreamClient::new(server.jetstream().await),
                SystemClock,
                &opentelemetry::global::meter("ext-method-name-round-trip"),
                Config::new(prefix.clone(), NatsConfig::from_url(server.address())),
            ));

            let (client_io, server_io) = tokio::io::duplex(8192);
            let (client_read, mut client_write) = tokio::io::split(client_io);
            let (server_read, server_write) = tokio::io::split(server_io);
            let boundary = connect_agent_boundary(
                bridge,
                async_compat::Compat::new(server_write),
                async_compat::Compat::new(server_read),
                async move |_cx| std::future::pending::<Result<()>>().await,
            );

            let observed = notifications.clone();
            let client = async move {
                let mut lines = BufReader::new(client_read).lines();
                send_frame(
                    &mut client_write,
                    &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": 1 } }),
                )
                .await;
                assert_eq!(read_frame(&mut lines).await["id"], 1);

                send_frame(
                    &mut client_write,
                    &json!({ "jsonrpc": "2.0", "method": EXT_METHOD, "params": { "note": true } }),
                )
                .await;
                timeout(WAIT, async {
                    while observed.names.lock().unwrap().is_empty() {
                        observed.arrived.notified().await;
                    }
                })
                .await
                .expect("ext notification never reached the agent");

                send_frame(
                    &mut client_write,
                    &json!({ "jsonrpc": "2.0", "id": 2, "method": EXT_METHOD, "params": { "steer": "left" } }),
                )
                .await;
                let response = read_frame(&mut lines).await;
                assert_eq!(response["id"], 2, "ext response correlation: {response}");
                assert_eq!(
                    response["result"],
                    json!({ "method": EXT_METHOD, "params": { "steer": "left" } }),
                    "ext response: {response}"
                );
                drop(client_write);
            };

            let (outcome, ()) = tokio::join!(boundary, client);
            assert!(matches!(outcome, Ok(BoundaryExit::TransportClosed)));
        })
        .await;

    assert_eq!(*notifications.names.lock().unwrap(), vec![EXT_METHOD.to_string()]);
}
