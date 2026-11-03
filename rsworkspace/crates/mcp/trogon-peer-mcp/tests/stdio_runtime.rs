use std::process::Stdio;
use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::{RoleClient, RunningService};
use serde_json::{Value, json};
use tokio::process::{Child, Command};
use trogon_decider_runtime::{ReadFrom, ReadStreamRequest, StreamRead};
use trogon_peer::{MessageId, PeerId};
use trogon_peer_nats::peer::{PeerJetStreamStore, PeerStorageBudget, PeerStoreConfig};
use trogon_peer_nats::test_support::{
    application,
    broker::{Broker, TestResult, observed_barrier},
};
use trogon_peer_nats::{PeerAccount, PeerTenant, serve_peer_rpc};
use trogon_peer_service::{WorkLimit, test_support::Fixture};
use trogonai_proto::peers::state_v1alpha1::PeerState;
use trogonai_proto::peers::v1alpha1 as v1;

struct StdioPeer {
    client: RunningService<RoleClient, ()>,
    child: Child,
}
impl StdioPeer {
    async fn start(broker: &Broker, identity: &str, connection: &str) -> TestResult<Self> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_trogon-peer-mcp"));
        command
            .env_clear()
            .kill_on_drop(true)
            .env("NATS_URL", broker.server.address())
            .env("NATS_CA", broker.server.ca())
            .env(
                "NATS_NKEY",
                format!("file://{}", broker.credential_path(identity)?.display()),
            )
            .env("PEER_TENANT_ID", "tenant")
            .env("PEER_ACCOUNT_MODE", "shared_fixture")
            .env("PEER_CALLER_ID", identity)
            .env("PEER_CONNECTION_ID", connection)
            .env("PEER_INSTALLATION_ID", "install")
            .env("PEER_COLLABORATION_REF", "shared")
            .env("PEER_DISPLAY_NAME", identity)
            .env("PEER_AVAILABILITY", "ready")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let output = child.stdout.take().ok_or("missing stdout")?;
        let input = child.stdin.take().ok_or("missing stdin")?;
        let client = tokio::time::timeout(Duration::from_secs(2), ().serve((output, input))).await??;
        Ok(Self { client, child })
    }
    async fn call(&self, name: &str, arguments: Value) -> TestResult<Value> {
        let request: CallToolRequestParams = serde_json::from_value(json!({"name":name,"arguments":arguments}))?;
        let result: CallToolResult = self.client.call_tool(request).await?;
        let value = serde_json::to_value(result)?;
        assert_ne!(value["isError"], true, "MCP operation failed");
        let text = value["content"]
            .as_array()
            .and_then(|values| values.first())
            .and_then(|v| v["text"].as_str());
        match text {
            Some(text) => Ok(serde_json::from_str(text)?),
            None => Ok(Value::Null),
        }
    }
    async fn prompt(&self, identity: &str, prompt: &str) -> TestResult<Value> {
        self.call(
            "peer_prompt_hook",
            json!({"session_id":identity,"prompt_id":prompt,"hook_event_name":"UserPromptSubmit"}),
        )
        .await
    }
    async fn finish(self) -> TestResult {
        self.client.cancel().await?;
        let output = tokio::time::timeout(Duration::from_secs(3), self.child.wait_with_output()).await??;
        assert!(output.status.success(), "connector did not exit cleanly");
        assert!(
            output.stderr.is_empty(),
            "connector stderr must be empty on a successful exchange"
        );
        Ok(())
    }
}
fn account() -> TestResult<PeerAccount> {
    Ok(PeerAccount::SharedFixture {
        tenant: PeerTenant::new("tenant")?,
    })
}
async fn history(store: &PeerJetStreamStore, peer: &PeerId) -> TestResult<PeerState> {
    let read = store
        .read_stream_bounded(
            ReadStreamRequest {
                stream_id: peer,
                from: ReadFrom::Beginning,
            },
            4096,
        )
        .await?;
    let mut state = trogon_peer::commands::state::initial_state();
    for event in read.events {
        let event = event.decode::<v1::PeerEvent>()?.into_decoded().ok_or("unknown event")?;
        state = trogon_peer::commands::state::evolve(state, &event)?;
    }
    Ok(state)
}

#[tokio::test]
async fn executable_stdio_authenticated_exchange_keeps_ack_explicit_and_detaches_on_eof() -> TestResult {
    let broker = Broker::start().await?;
    let (service, _) = broker.connect("service").await?;
    let fixture = Fixture::new().await?;
    let store = PeerJetStreamStore::open(
        async_nats::jetstream::new(service.clone()),
        &PeerStoreConfig::new(PeerTenant::new("tenant")?, PeerStorageBudget::new(16_777_216)?),
    )
    .await?;
    let backend = application::compose(store.clone(), &fixture);
    let registration = serve_peer_rpc(&service, account()?, backend.clone()).await?;
    observed_barrier(&service).await?;
    let alice = StdioPeer::start(&broker, "alice", "one").await?;
    let bob = StdioPeer::start(&broker, "bob", "one").await?;
    let tools = alice.client.list_all_tools().await?;
    assert_eq!(tools.len(), 6);
    assert!(
        !tools
            .iter()
            .any(|tool| tool.name.contains("pull") || tool.name.contains("read_body"))
    );
    alice.prompt("alice", "initial").await?;
    bob.prompt("bob", "initial").await?;
    let sender = PeerId::parse(fixture.a.peer_id.clone().ok_or("sender")?)?;
    let recipient = PeerId::parse(fixture.b.peer_id.clone().ok_or("recipient")?)?;
    let body = "private executable exchange <payload>";
    let request = json!({"recipient_peer_id":recipient.as_str(), "idempotency_key":"stable_send", "text":body});
    let sent = alice.call("peer_send", request.clone()).await?;
    assert!(!sent.to_string().contains(body));
    assert_eq!(sent, alice.call("peer_send", request).await?);
    assert_eq!(history(&store, &sender).await?.outbox.len(), 1);
    let message = history(&store, &sender).await?.outbox[0]
        .message
        .message_id
        .clone()
        .ok_or("message")?;
    backend
        .application()
        .reconcile(&fixture.alice, &sender, &[MessageId::new(message)?], WorkLimit::new(1)?)
        .await?;
    let context = bob.prompt("bob", "delivery").await?;
    assert!(
        context["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .is_some_and(|s| s.contains("private executable exchange"))
    );
    assert_eq!(context, bob.prompt("bob", "delivery").await?);
    let state = history(&store, &recipient).await?;
    assert!(!state.batches[0].acknowledged_at.is_set());
    let batch = state.batches[0].batch_id.clone().ok_or("batch")?;
    let ack = bob.call("peer_acknowledge_batch", json!({"batch_id":batch})).await?;
    assert!(!ack.to_string().contains(body));
    let status = alice
        .call("peer_status", json!({"idempotency_key":"stable_send"}))
        .await?;
    assert!(!status.to_string().contains(body));
    assert_eq!(status["status"], "MESSAGE_STATUS_APPLICATION_ACKNOWLEDGED");
    let roster = alice.call("peer_roster", json!({})).await?;
    assert!(!roster.to_string().contains(body));
    bob.call(
        "peer_detach_hook",
        json!({"session_id":"bob", "hook_event_name":"SessionEnd"}),
    )
    .await?;
    bob.finish().await?;
    alice.finish().await?;
    for peer in [&sender, &recipient] {
        let state = history(&store, peer).await?;
        assert_eq!(
            state.attachment.status.and_then(|v| v.as_known()),
            Some(v1::AttachmentStatus::ATTACHMENT_STATUS_DETACHED)
        );
        assert_eq!(state.attachment.attachment.generation, Some(1));
    }
    assert_eq!(history(&store, &recipient).await?.inbox.len(), 1);
    registration.stop().await?;
    Ok(())
}
