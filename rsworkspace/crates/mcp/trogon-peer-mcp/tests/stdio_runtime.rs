use futures::StreamExt;
use std::process::Stdio;
use std::time::Duration;
use trogon_std::env::ReadEnv;
use trogon_std::env::SystemEnv;

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
        Self::start_at(broker, identity, connection, broker.server.address()).await
    }
    async fn start_at(broker: &Broker, identity: &str, connection: &str, address: &str) -> TestResult<Self> {
        Self::start_at_with_routing(broker, identity, identity, connection, address).await
    }
    async fn start_with_routing(broker: &Broker, identity: &str, caller: &str, connection: &str) -> TestResult<Self> {
        Self::start_at_with_routing(broker, identity, caller, connection, broker.server.address()).await
    }
    async fn start_at_with_routing(
        broker: &Broker,
        identity: &str,
        caller: &str,
        connection: &str,
        address: &str,
    ) -> TestResult<Self> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_trogon-peer-mcp"));
        command
            .env_clear()
            .kill_on_drop(true)
            .env("RUST_LOG", "trace")
            .env("NATS_URL", address)
            .env("NATS_CA", broker.server.ca())
            .env(
                "NATS_NKEY",
                format!("file://{}", broker.credential_path(identity)?.display()),
            )
            .env("PEER_TENANT_ID", "tenant")
            .env("PEER_ACCOUNT_MODE", "shared_fixture")
            .env("PEER_CALLER_ID", caller)
            .env("PEER_CONNECTION_ID", connection)
            .env("PEER_INSTALLATION_ID", "install")
            .env("PEER_COLLABORATION_REF", "shared")
            .env("PEER_DISPLAY_NAME", identity)
            .env("PEER_AVAILABILITY", "ready")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(profile) = SystemEnv.var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
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
        assert_ne!(value["isError"], true, "MCP operation {name} failed: {value}");
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
        assert_eq!(state.attachment.attachment_ref.generation, Some(1));
    }
    assert_eq!(history(&store, &recipient).await?.inbox.len(), 1);
    registration.stop().await?;
    Ok(())
}

struct BrokerOutage {
    address: String,
    disconnect: tokio::sync::mpsc::Sender<()>,
    recovered: tokio::sync::mpsc::Receiver<()>,
    task: tokio::task::JoinHandle<TestResult>,
}

impl BrokerOutage {
    async fn start(upstream: &str) -> TestResult<Self> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?.to_string();
        let upstream = upstream.to_owned();
        let (disconnect, mut disconnected) = tokio::sync::mpsc::channel(1);
        let (resumed, recovered) = tokio::sync::mpsc::channel(1);
        let task = tokio::spawn(async move {
            let (mut incoming, _) = listener.accept().await?;
            let mut outgoing = tokio::net::TcpStream::connect(&upstream).await?;
            for _ in 0..2 {
                tokio::select! {
                    result = tokio::io::copy_bidirectional(&mut incoming, &mut outgoing) => {
                        result?;
                        return Err("session closed before requested fixture outage".into());
                    },
                    result = disconnected.recv() => {
                        result.ok_or("fixture outage request channel closed")?;
                    },
                }
                drop(incoming);
                drop(outgoing);
                let (rejected, _) = listener.accept().await?;
                drop(rejected);
                let (next, _) = listener.accept().await?;
                incoming = next;
                outgoing = tokio::net::TcpStream::connect(&upstream).await?;
                resumed.send(()).await.map_err(|_| "fixture recovery receiver closed")?;
            }
            let _closed = tokio::io::copy_bidirectional(&mut incoming, &mut outgoing).await;
            Ok(())
        });
        Ok(Self {
            address,
            disconnect,
            recovered,
            task,
        })
    }
}

#[tokio::test]
async fn executable_session_recovers_after_two_failed_authenticated_reconnects() -> TestResult {
    let broker = Broker::start().await?;
    let (service, _) = broker.connect("service").await?;
    let fixture = Fixture::new().await?;
    let store = PeerJetStreamStore::open(
        async_nats::jetstream::new(service.clone()),
        &PeerStoreConfig::new(PeerTenant::new("tenant")?, PeerStorageBudget::new(16_777_216)?),
    )
    .await?;
    let backend = application::compose(store, &fixture);
    let registration = serve_peer_rpc(&service, account()?, backend).await?;
    observed_barrier(&service).await?;
    let mut proxy = BrokerOutage::start(broker.server.address()).await?;
    let alice = StdioPeer::start_at(&broker, "alice", "one", &proxy.address).await?;
    let bob = StdioPeer::start(&broker, "bob", "one").await?;
    bob.prompt("bob", "catalog_member").await?;
    let recovered: TestResult = async {
        alice.prompt("alice", "before_outage").await?;
        let before = alice.call("peer_roster", json!({})).await?;
        assert_eq!(before["peers"].as_array().ok_or("roster absent")?.len(), 2);
        for prompt in ["after_first_outage", "after_second_outage"] {
            proxy
                .disconnect
                .send(())
                .await
                .map_err(|_| "fixture relay closed before outage")?;
            tokio::time::timeout(Duration::from_secs(12), proxy.recovered.recv())
                .await
                .map_err(|error| {
                    std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!("authenticated session stopped reconnecting after a rejected dial: {error}"),
                    )
                })?
                .ok_or("session stopped reconnecting before the broker recovered")?;
            assert_eq!(
                alice.call("peer_roster", json!({})).await?,
                before,
                "the same authenticated session must retain its roster after recovery"
            );
            alice.prompt("alice", prompt).await?;
        }
        alice.finish().await?;
        bob.finish().await?;
        Ok(())
    }
    .await;
    if recovered.is_err() {
        proxy.task.abort();
    }
    let relay = proxy.task.await;
    let stopped = registration.stop().await;
    recovered?;
    relay??;
    stopped?;
    Ok(())
}

#[tokio::test]
async fn executable_reports_uncertain_shutdown_when_bound_detach_has_no_service() -> TestResult {
    let broker = Broker::start().await?;
    let (service, _) = broker.connect("service").await?;
    let fixture = Fixture::new().await?;
    let store = PeerJetStreamStore::open(
        async_nats::jetstream::new(service.clone()),
        &PeerStoreConfig::new(PeerTenant::new("tenant")?, PeerStorageBudget::new(16_777_216)?),
    )
    .await?;
    let registration = serve_peer_rpc(&service, account()?, application::compose(store, &fixture)).await?;
    observed_barrier(&service).await?;
    let peer = StdioPeer::start(&broker, "alice", "one").await?;
    peer.prompt("alice", "before_service_stops").await?;
    registration.stop().await?;
    observed_barrier(&service).await?;
    tokio::time::timeout(Duration::from_secs(3), peer.client.cancel()).await??;
    let output = tokio::time::timeout(Duration::from_secs(3), peer.child.wait_with_output()).await??;
    assert!(
        !output.status.success(),
        "unconfirmed detach was reported as a successful exit"
    );
    let diagnostic = String::from_utf8(output.stderr)?;
    assert!(
        diagnostic.contains("remote outcomes remain uncertain"),
        "shutdown uncertainty was not reported"
    );
    assert!(
        !diagnostic.contains("Peer connector could not start."),
        "bound detach failure was labeled startup"
    );
    assert!(
        !diagnostic.contains("alice"),
        "shutdown diagnostic exposed caller routing"
    );
    Ok(())
}

#[tokio::test]
async fn executable_credentials_cannot_publish_another_caller_or_read_streams() -> TestResult {
    let broker = Broker::start().await?;
    let (service, _) = broker.connect("service").await?;
    let mut forbidden = service.subscribe("tenant.peer.v1.bob.PeerService.Enroll").await?;
    observed_barrier(&service).await?;
    let wrong = StdioPeer::start_with_routing(&broker, "alice", "bob", "one").await?;
    let request: CallToolRequestParams = serde_json::from_value(
        json!({"name":"peer_prompt_hook", "arguments":{"session_id":"bob", "hook_event_name":"UserPromptSubmit"}}),
    )?;
    let result = wrong.client.call_tool(request).await?;
    assert_eq!(result.is_error, Some(true));
    wrong.client.cancel().await?;
    let output = tokio::time::timeout(Duration::from_secs(3), wrong.child.wait_with_output()).await??;
    assert!(
        !output.status.success(),
        "an unconfirmed enrollment must not claim clean detach"
    );
    assert_eq!(
        String::from_utf8(output.stderr)?,
        "Peer connector stopped without confirming clean shutdown; remote outcomes remain uncertain.\n"
    );
    let (client, mut errors) = broker.connect("alice").await?;
    client
        .publish("tenant.peer.v1.bob.PeerService.Enroll", vec![0].into())
        .await?;
    client.flush().await?;
    let error = tokio::time::timeout(Duration::from_secs(2), errors.recv())
        .await?
        .ok_or("no broker denial")?;
    assert_eq!(
        error,
        async_nats::ServerError::Other(
            "Permissions Violation for Publish to \"tenant.peer.v1.bob.PeerService.Enroll\"".into()
        )
    );
    let _subscription = client.subscribe("$JS.API.>").await?;
    client.flush().await?;
    let error = tokio::time::timeout(Duration::from_secs(2), errors.recv())
        .await?
        .ok_or("no subscription denial")?;
    assert_eq!(
        error,
        async_nats::ServerError::Other("Permissions Violation for Subscription to \"$JS.API.>\"".into())
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), forbidden.next())
            .await
            .is_err()
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn executable_sigterm_detaches_bound_generation_without_durable_erasure() -> TestResult {
    let broker = Broker::start().await?;
    let (service, _) = broker.connect("service").await?;
    let fixture = Fixture::new().await?;
    let store = PeerJetStreamStore::open(
        async_nats::jetstream::new(service.clone()),
        &PeerStoreConfig::new(PeerTenant::new("tenant")?, PeerStorageBudget::new(16_777_216)?),
    )
    .await?;
    let registration = serve_peer_rpc(&service, account()?, application::compose(store.clone(), &fixture)).await?;
    observed_barrier(&service).await?;
    let peer = StdioPeer::start(&broker, "alice", "one").await?;
    peer.prompt("alice", "initial").await?;
    let id = peer.child.id().ok_or("missing process")?;
    let status = Command::new("kill").arg("-TERM").arg(id.to_string()).status().await?;
    assert!(status.success());
    let output = tokio::time::timeout(Duration::from_secs(3), peer.child.wait_with_output()).await??;
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    peer.client.cancel().await?;
    let sender = PeerId::parse(fixture.a.peer_id.clone().ok_or("peer")?)?;
    let state = history(&store, &sender).await?;
    assert!(state.enrollment.is_set());
    assert_eq!(
        state.attachment.status.and_then(|v| v.as_known()),
        Some(v1::AttachmentStatus::ATTACHMENT_STATUS_DETACHED)
    );
    assert_eq!(state.attachment.attachment_ref.generation, Some(1));
    registration.stop().await?;
    Ok(())
}

#[cfg(unix)]
async fn public_cli_signal_preserves_prepared_assignment(signal: &str) -> TestResult {
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
    alice.prompt("alice", "initial").await?;
    bob.prompt("bob", "initial").await?;
    let sender = PeerId::parse(fixture.a.peer_id.clone().ok_or("sender")?)?;
    let recipient = PeerId::parse(fixture.b.peer_id.clone().ok_or("recipient")?)?;
    let body = "prepared context must survive native signal shutdown";
    let sent = alice
        .call(
            "peer_send",
            json!({"recipient_peer_id":recipient.as_str(),"idempotency_key":"signal_assignment","text":body}),
        )
        .await?;
    assert!(!sent.to_string().contains(body));
    let before_intake = history(&store, &sender).await?;
    assert_eq!(before_intake.outbox.len(), 1);
    let message = MessageId::new(before_intake.outbox[0].message.message_id.clone().ok_or("message")?)?;
    backend
        .application()
        .reconcile(&fixture.alice, &sender, &[message], WorkLimit::new(1)?)
        .await?;
    let context = bob.prompt("bob", "delivery").await?;
    assert!(
        context["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .is_some_and(|value| value.contains(body))
    );
    let prepared = history(&store, &recipient).await?;
    assert_eq!(prepared.inbox.len(), 1);
    assert_eq!(prepared.batches.len(), 1);
    assert!(!prepared.batches[0].acknowledged_at.is_set());
    assert_eq!(prepared.attachment.attachment_ref.generation, Some(1));
    assert_eq!(
        prepared.attachment.status.and_then(|value| value.as_known()),
        Some(v1::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED)
    );
    let pending_sender = history(&store, &sender).await?;
    assert_eq!(pending_sender.outbox.len(), 1);
    assert!(!pending_sender.outbox[0].settlement.is_set());
    let pid = bob.child.id().ok_or("missing owned connector process")?;
    let delivered = tokio::time::timeout(
        Duration::from_secs(2),
        Command::new("kill").arg(signal).arg(pid.to_string()).status(),
    )
    .await??;
    assert!(delivered.success(), "signal was not delivered to owned connector");
    let output = tokio::time::timeout(Duration::from_secs(3), bob.child.wait_with_output())
        .await
        .map_err(|_| "owned signalled connector exit timed out")??;
    assert!(output.status.success(), "connector did not handle {signal} gracefully");
    assert!(
        output.stderr.is_empty(),
        "successful signal shutdown wrote a diagnostic"
    );
    tokio::time::timeout(Duration::from_secs(3), bob.client.cancel())
        .await
        .map_err(|_| "signalled connector client cancellation timed out")??;
    let detached = history(&store, &recipient).await?;
    assert_eq!(detached.enrollment, prepared.enrollment);
    assert_eq!(detached.attachment.attachment_ref, prepared.attachment.attachment_ref);
    assert_eq!(
        detached.attachment.status.and_then(|value| value.as_known()),
        Some(v1::AttachmentStatus::ATTACHMENT_STATUS_DETACHED)
    );
    assert_eq!(detached.inbox, prepared.inbox);
    assert_eq!(detached.dispositions, prepared.dispositions);
    assert_eq!(detached.batches, prepared.batches);
    assert!(!detached.batches[0].acknowledged_at.is_set());
    assert_eq!(history(&store, &sender).await?.outbox, pending_sender.outbox);
    alice.finish().await?;
    registration.stop().await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn executable_sigterm_detaches_prepared_assignment_without_acknowledgment() -> TestResult {
    public_cli_signal_preserves_prepared_assignment("-TERM").await
}

#[cfg(unix)]
#[tokio::test]
async fn executable_sigint_detaches_prepared_assignment_without_acknowledgment() -> TestResult {
    public_cli_signal_preserves_prepared_assignment("-INT").await
}

#[tokio::test]
async fn executable_invalid_settings_keep_stdout_empty_and_diagnostics_static() -> TestResult {
    let output = Command::new(env!("CARGO_BIN_EXE_trogon-peer-mcp"))
        .env_clear()
        .envs(
            SystemEnv
                .var_os("LLVM_PROFILE_FILE")
                .map(|profile| ("LLVM_PROFILE_FILE", profile)),
        )
        .output()
        .await?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr)?,
        "Peer connector settings are invalid or credentials are missing.\n"
    );
    Ok(())
}
