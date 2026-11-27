//! A child's extension calls cross the host and NATS to the client bridge with
//! their exact name, and the client's answer comes back to the child.

use super::*;
use crate::initialized_agent::InitializedAgent;
use crate::supervisor::{LaunchAgent, RunStop, run_once};
use acp_nats::boundary::{ConnectionAgent, connect_agent_boundary};
use acp_nats::{AgentHandler, Bridge, Config, NatsConfig};
use agent_client_protocol::v1::schema::{
    AuthenticateRequest, AuthenticateResponse, CancelNotification, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, RequestPermissionOutcome, StopReason,
};
use agent_client_protocol::{ByteStreams, UntypedMessage};
use async_compat::Compat;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::io::{DuplexStream, ReadHalf, WriteHalf};
use tokio::sync::{Notify, oneshot};
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_std::time::SystemClock;

const PREFIX: &str = "ext_round_trip";

/// The client at the far end of the client bridge.
#[derive(Default)]
struct RecordingClient {
    requests: Mutex<Vec<(String, serde_json::Value)>>,
    notifications: Mutex<Vec<(String, serde_json::Value)>>,
    notified: Notify,
}

#[async_trait::async_trait]
impl ClientHandler for RecordingClient {
    async fn request_permission(&self, _args: RequestPermissionRequest) -> Result<RequestPermissionResponse> {
        Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
    }

    async fn session_notification(&self, _args: SessionNotification) -> Result<()> {
        Ok(())
    }

    async fn ext_method(&self, args: ExtRequest) -> Result<ExtResponse> {
        let params = serde_json::from_str(args.params.get()).expect("params are JSON");
        self.requests.lock().unwrap().push((args.method.to_string(), params));
        Ok(ExtResponse::new(raw(r#"{"answer":42}"#)))
    }

    async fn ext_notification(&self, args: ExtNotification) -> Result<()> {
        let params = serde_json::from_str(args.params.get()).expect("params are JSON");
        self.notifications
            .lock()
            .unwrap()
            .push((args.method.to_string(), params));
        self.notified.notify_one();
        Ok(())
    }
}

/// The child: once launched, it calls `_vendor/x` on its client and then
/// notifies `_vendor/x`, and hands the test the answer it got.
struct ExtCallingLauncher {
    answer: Mutex<Option<oneshot::Sender<serde_json::Value>>>,
}

struct IdleAgent;

#[async_trait::async_trait]
impl AgentHandler for IdleAgent {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        Ok(InitializeResponse::new(args.protocol_version))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, _args: NewSessionRequest) -> Result<NewSessionResponse> {
        Ok(NewSessionResponse::new("s1"))
    }

    async fn prompt(&self, _args: PromptRequest) -> Result<PromptResponse> {
        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn cancel(&self, _args: CancelNotification) -> Result<()> {
        Ok(())
    }
}

impl LaunchAgent for ExtCallingLauncher {
    type Transport = ByteStreams<Compat<WriteHalf<DuplexStream>>, Compat<ReadHalf<DuplexStream>>>;

    fn launch(&self) -> Self::Transport {
        let (host_io, agent_io) = tokio::io::duplex(64 * 1024);
        let (host_read, host_write) = tokio::io::split(host_io);
        let (agent_read, agent_write) = tokio::io::split(agent_io);
        let answer = self.answer.lock().unwrap().take().expect("launched once");
        tokio::spawn(connect_agent_boundary(
            Arc::new(IdleAgent),
            Compat::new(agent_write),
            Compat::new(agent_read),
            async move |cx| {
                let request = json!({"sessionId": "s1", "question": 1});
                // The client bridge subscribes on its own task; until it has,
                // NATS has no responder for the call.
                let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
                let reply = loop {
                    let call = cx
                        .send_request(UntypedMessage::new("_vendor/x", &request)?)
                        .block_task()
                        .await;
                    match call {
                        Ok(reply) => break reply,
                        Err(_) if tokio::time::Instant::now() < deadline => {
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                        Err(error) => return Err(error),
                    }
                };
                cx.send_notification(UntypedMessage::new(
                    "_vendor/x",
                    json!({"sessionId": "s1", "note": true}),
                )?)?;
                let _ = answer.send(reply);
                std::future::pending::<Result<()>>().await
            },
        ));
        ByteStreams::new(Compat::new(host_write), Compat::new(host_read))
    }
}

#[tokio::test]
async fn a_childs_extension_calls_reach_the_client_by_exact_name_and_the_answer_comes_back() {
    let server = JetStreamTestServer::start().await;
    let nats = server.client().await;
    let prefix = AcpPrefix::new(PREFIX).unwrap();

    let client = Arc::new(RecordingClient::default());
    let bridge = Arc::new(Bridge::new(
        nats.clone(),
        (),
        SystemClock,
        &opentelemetry::global::meter("acp-host-ext-round-trip"),
        Config::new(prefix.clone(), NatsConfig::from_url(server.address())),
    ));
    tokio::spawn(acp_nats::client::run(nats.clone(), client.clone(), bridge));

    let (answer_tx, answer_rx) = oneshot::channel();
    let launcher = ExtCallingLauncher {
        answer: Mutex::new(Some(answer_tx)),
    };
    let host = Arc::new(SessionRoutedClient::new(nats.clone(), prefix, Duration::from_secs(5)));
    let serve = async |_agent: InitializedAgent<ConnectionAgent>, stop: RunStop| {
        stop.await;
        Ok::<(), agent_client_protocol::Error>(())
    };
    let shutdown = CancellationToken::new();
    let run = run_once(&launcher, host, Arc::default(), Arc::default(), &serve, &shutdown);

    let answer = tokio::select! {
        exit = run => panic!("the run ended: {exit}"),
        answer = tokio::time::timeout(Duration::from_secs(15), async {
            let answer = answer_rx.await.expect("the child got an answer");
            client.notified.notified().await;
            answer
        }) => answer.expect("the round trip finishes"),
    };

    assert_eq!(answer, json!({"answer": 42}));
    assert_eq!(
        *client.requests.lock().unwrap(),
        vec![("_vendor/x".to_string(), json!({"sessionId": "s1", "question": 1}))]
    );
    assert_eq!(
        *client.notifications.lock().unwrap(),
        vec![("_vendor/x".to_string(), json!({"sessionId": "s1", "note": true}))]
    );
}
