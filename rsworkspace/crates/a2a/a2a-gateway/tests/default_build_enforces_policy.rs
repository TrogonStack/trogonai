//! End-to-end proof that the default build (no `spicedb` feature) enforces
//! policy: the real gateway runtime runs against a real NATS server with a
//! Tier-1 declarative bundle, a request the bundle allows reaches the agent,
//! and a request it denies is refused, never forwarded, and audited.
//!
//! The ingress subject is `{prefix}.v1.gateway.{agent}.{kind}.{id}.{method}`;
//! the `{kind}.{id}` pair is the only caller principal the gateway honors
//! (ADR#0082), so there is no anonymous path to exercise.

use std::path::Path;
use std::time::Duration;

use a2a_gateway::Args;
use a2a_gateway::runtime::run_with_args;
use futures::{StreamExt, TryStreamExt};
use tokio::task::JoinHandle;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_std::env::InMemoryEnv;

const ALLOWED_INGRESS: &str = "a2a.v1.gateway.planner.person.alice.message.send";
const DENIED_INGRESS: &str = "a2a.v1.gateway.planner.person.alice.tasks.cancel";
const UNROUTABLE_INGRESS: &str = "a2a.v1.gateway.planner.person.alice.unknown";
const FORWARDED_SUBJECT: &str = "a2a.v1.agents.planner.person.alice.message.send";
const AGENT_WILDCARD: &str = "a2a.v1.agents.planner.>";
const AUDIT_ERR: &str = "a2a.v1.audit.planner.err";
const AUDIT_STREAM: &str = "AUDIT";
const AUDIT_STREAM_SUBJECTS: &str = "platform.v1.audit.>";
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// Allow `message/send`; the catch-all denies every other method.
const BUNDLE: &str = r#"
[[rule]]
id = "allow-message-send"
priority = 200
effect = "allow"

[[rule.matches]]
kind = "agent_method"
pattern = "message/send"

[[rule]]
id = "deny-other-methods"
priority = 100
effect = "deny"

[[rule.matches]]
kind = "agent_method"
pattern = "*"
"#;

#[derive(Debug, thiserror::Error)]
enum FixtureError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Timeout(#[from] tokio::time::error::Elapsed),
    #[error(transparent)]
    Connect(#[from] async_nats::ConnectError),
    #[error(transparent)]
    Subscribe(#[from] async_nats::SubscribeError),
    #[error(transparent)]
    CreateStream(#[from] async_nats::jetstream::context::CreateStreamError),
    #[error(transparent)]
    StreamInfo(#[from] async_nats::jetstream::context::RequestError),
    #[error(transparent)]
    Flush(#[from] async_nats::client::FlushError),
    #[error(transparent)]
    Publish(#[from] async_nats::client::PublishError),
    #[error(transparent)]
    Request(#[from] async_nats::RequestError),
    #[error(transparent)]
    Nkey(#[from] nkeys::error::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
    #[error("gateway stopped before serving: {0:?}")]
    GatewayStopped(Result<(), String>),
    #[error("subscription closed before a message arrived")]
    SubscriptionClosed,
    #[error("forwarded request carried no reply inbox")]
    NoReplyInbox,
}

/// Aborts the gateway task on every exit path, including a failed assertion.
struct GatewayTask(JoinHandle<Result<(), String>>);

impl Drop for GatewayTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn gateway_env(bundle_dir: &Path) -> Result<InMemoryEnv, FixtureError> {
    let env = InMemoryEnv::new();
    env.set("NATS_TLS_DISABLED", "true");
    env.set("AUTH_CALLOUT_SIGNING_SECRET", nkeys::KeyPair::new_account().seed()?);
    env.set("A2A_GATEWAY_TIER1_DECLARATIVE_ENABLED", "true");
    env.set("A2A_GATEWAY_TIER1_BUNDLE_DIR", bundle_dir.to_string_lossy());
    env.set("A2A_GATEWAY_AUDIT_PUBLISH", "true");
    env.set("A2A_GATEWAY_AUDIT_TENANT", "acme");
    env.set("A2A_GATEWAY_AUDIT_PROJECT", "payments");
    env.set("A2A_GATEWAY_AUDIT_INSTANCE", "gw-1");
    Ok(env)
}

fn json_rpc(id: &str, method: &str) -> bytes::Bytes {
    bytes::Bytes::from(
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": {"probe": id}}).to_string(),
    )
}

async fn next_message(subscriber: &mut async_nats::Subscriber) -> Result<async_nats::Message, FixtureError> {
    tokio::time::timeout(REPLY_TIMEOUT, subscriber.next())
        .await?
        .ok_or(FixtureError::SubscriptionClosed)
}

/// Send `message/send` through the gateway and return the caller-visible reply.
async fn send_allowed(client: async_nats::Client, id: &'static str) -> Result<serde_json::Value, FixtureError> {
    let reply = client.request(ALLOWED_INGRESS, json_rpc(id, "message/send")).await?;
    Ok(serde_json::from_slice(&reply.payload)?)
}

/// Play the downstream agent: answer a forwarded request on the caller's inbox.
async fn answer_forwarded(client: &async_nats::Client, forwarded: &async_nats::Message) -> Result<(), FixtureError> {
    let reply = forwarded.reply.clone().ok_or(FixtureError::NoReplyInbox)?;
    let request: serde_json::Value = serde_json::from_slice(&forwarded.payload)?;
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": request["id"],
        "result": {"handled_by": "fake-agent"},
    });
    client.publish(reply, body.to_string().into()).await?;
    Ok(())
}

#[tokio::test]
async fn the_default_build_forwards_allowed_requests_and_refuses_denied_ones() -> Result<(), FixtureError> {
    // `InMemoryEnv` is not `Sync`, so the runtime future is not `Send`; it
    // runs on the test thread's local set instead of a worker thread.
    tokio::task::LocalSet::new()
        .run_until(enforce_policy_end_to_end())
        .await
}

async fn enforce_policy_end_to_end() -> Result<(), FixtureError> {
    let server = JetStreamTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;

    // Audit publishing writes every allow that acts ahead of the forward, so
    // the platform audit stream must exist before the gateway serves.
    let audit_stream = server
        .jetstream()
        .await
        .create_stream(async_nats::jetstream::stream::Config {
            name: AUDIT_STREAM.into(),
            subjects: vec![AUDIT_STREAM_SUBJECTS.into()],
            ..Default::default()
        })
        .await?;

    // The fake downstream agent and the audit listener subscribe before the
    // gateway starts, so nothing the gateway publishes can be missed.
    let mut agent = client.subscribe(AGENT_WILDCARD).await?;
    let mut audit = client.subscribe(AUDIT_ERR).await?;
    client.flush().await?;

    let bundle_dir = tempfile::tempdir()?;
    std::fs::write(bundle_dir.path().join("policy.tier1.toml"), BUNDLE)?;
    let env = gateway_env(bundle_dir.path())?;
    let args = Args {
        nats_url: server.address().to_owned(),
        prefix: "a2a".to_owned(),
        queue_group: None,
    };
    let mut gateway = GatewayTask(tokio::task::spawn_local(async move {
        run_with_args(args, &env).await.map_err(|error| error.to_string())
    }));

    // Ready once the gateway answers an unroutable method with an invalid
    // request error; that path is rejected before any policy tier or forward.
    tokio::time::timeout(REPLY_TIMEOUT, async {
        loop {
            if client
                .request(UNROUTABLE_INGRESS, json_rpc("startup", "unknown"))
                .await
                .is_ok()
            {
                return Ok(());
            }
            if gateway.0.is_finished() {
                return Err(FixtureError::GatewayStopped((&mut gateway.0).await?));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;

    // Allowed: forwarded to the agent subject, and the caller gets the agent's reply.
    let first = tokio::spawn(send_allowed(client.clone(), "allow-1"));
    let forwarded = next_message(&mut agent).await?;
    assert_eq!(forwarded.subject.as_str(), FORWARDED_SUBJECT);
    let forwarded_body: serde_json::Value = serde_json::from_slice(&forwarded.payload)?;
    assert_eq!(forwarded_body["id"], "allow-1");
    assert_eq!(forwarded_body["method"], "message/send");
    answer_forwarded(&client, &forwarded).await?;
    let first = first.await??;
    assert_eq!(first["id"], "allow-1");
    assert_eq!(first["result"]["handled_by"], "fake-agent");

    // Denied: JSON-RPC -32803 back to the caller, and an audit event follows.
    let denied = client
        .request(DENIED_INGRESS, json_rpc("deny-1", "tasks/cancel"))
        .await?;
    let denied: serde_json::Value = serde_json::from_slice(&denied.payload)?;
    assert_eq!(denied["jsonrpc"], "2.0");
    assert_eq!(denied["id"], "deny-1");
    assert_eq!(denied["error"]["code"], -32_803);
    assert_eq!(
        denied["error"]["message"],
        "tier-1 declarative policy rejected envelope"
    );
    assert!(
        denied.get("result").is_none(),
        "a denial must carry no result: {denied}"
    );

    let audit_event: serde_json::Value = serde_json::from_slice(&next_message(&mut audit).await?.payload)?;
    assert_eq!(audit_event["agent_id"], "planner");
    assert_eq!(audit_event["method"], "tasks/cancel");
    assert_eq!(audit_event["req_id"], "deny-1");
    assert_eq!(audit_event["outcome"], "err");
    assert_eq!(audit_event["code"], -32_803);
    assert_eq!(audit_event["tier1_decision"], "deny");
    assert_eq!(audit_event["caller_id"], "person.alice");
    assert_eq!(
        audit_event["rules_fired"],
        serde_json::json!(["gateway.tier1.declarative.denied.deny-other-methods"])
    );

    // Not forwarded: the gateway dispatches ingress in order over one
    // connection, so a forward of the denied request would reach the agent
    // before this follow-up does. The agent's next message must be the follow-up.
    let second = tokio::spawn(send_allowed(client.clone(), "allow-2"));
    let forwarded = next_message(&mut agent).await?;
    assert_eq!(forwarded.subject.as_str(), FORWARDED_SUBJECT);
    let forwarded_body: serde_json::Value = serde_json::from_slice(&forwarded.payload)?;
    assert_eq!(
        forwarded_body["id"], "allow-2",
        "the denied request must never reach the agent"
    );
    answer_forwarded(&client, &forwarded).await?;
    let second = second.await??;
    assert_eq!(second["result"]["handled_by"], "fake-agent");

    // Only the denial was audited as an error.
    assert!(
        tokio::time::timeout(Duration::from_millis(200), audit.next())
            .await
            .is_err(),
        "the allowed requests must not produce an error audit event"
    );

    // Both allows were written ahead of their forward, the deny was recorded,
    // and the writer opened its chain with a reset marker at startup.
    let recorded: std::collections::BTreeMap<String, usize> = audit_stream
        .info_with_subjects(AUDIT_STREAM_SUBJECTS)
        .await?
        .try_collect()
        .await?;
    assert_eq!(
        recorded,
        std::collections::BTreeMap::from([
            ("platform.v1.audit.decision.payments.allow".to_owned(), 2),
            ("platform.v1.audit.decision.payments.deny".to_owned(), 1),
            ("platform.v1.audit.integrity.a2a_gateway.chain_reset".to_owned(), 1),
        ])
    );
    Ok(())
}
