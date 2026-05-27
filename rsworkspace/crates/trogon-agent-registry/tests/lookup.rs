//! Integration test against a live NATS broker (`#[ignore]`).

use std::time::Duration;

use futures::StreamExt;
use trogon_agent_registry::{
    AUDIT_LOOKUP_FOUND, AgentRecord, AgentRegistryStore, LOOKUP_SUBJECT, LifecycleState, LookupRequest, LookupResponse,
    RegistryCache, lookup, open_bucket, run_lookup_consumer,
};
use trogon_nats::{NatsAuth, NatsConfig, connect};
use uuid::Uuid;

fn sample_record(agent_id: &str) -> AgentRecord {
    AgentRecord {
        agent_id: agent_id.to_string(),
        agent_version: "1.0.0".to_string(),
        agent_definition_digest: "sha256:integration".to_string(),
        owner_team: "platform".to_string(),
        allowed_workloads: vec!["spiffe://acme.local/ns/prod/sa/oncall-agent".to_string()],
        allowed_tools: vec!["pagerduty.page".to_string()],
        allowed_audiences: vec!["mcp.server.pagerduty".to_string()],
        allowed_purposes: Some(vec!["incident.response".to_string()]),
        mesh_token_ttl_s: Some(300),
        metadata: serde_json::json!({"description": "integration test agent"}),
        lifecycle_state: LifecycleState::Active,
        created_at: "2026-05-27T00:00:00Z".to_string(),
        updated_at: "2026-05-27T00:00:00Z".to_string(),
    }
}

#[tokio::test]
#[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-agent-registry -- --ignored"]
async fn lookup_returns_found_record_and_emits_audit_event() {
    let url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let connect_timeout = Duration::from_secs(15);
    let nats_conf = NatsConfig::new(vec![url], NatsAuth::None);

    let client = connect(&nats_conf, connect_timeout).await.expect("connect nats");
    let jetstream = async_nats::jetstream::new(client.clone());

    let kv = open_bucket(&jetstream, true).await.expect("open registry bucket");
    let store = AgentRegistryStore::new(kv, client.clone());
    let cache = RegistryCache::new();

    let suffix = Uuid::now_v7().as_simple().to_string();
    let agent_id = format!("acme/test-agent-{suffix}");
    let record = sample_record(&agent_id);
    store.put(record.clone()).await.expect("seed registry record");
    store.warm_cache(cache.clone()).await.expect("warm cache");

    let mut audit_sub = client
        .subscribe(AUDIT_LOOKUP_FOUND.to_string())
        .await
        .expect("subscribe audit subject");

    let consumer_client = client.clone();
    let consumer_store = store.clone();
    let consumer_cache = cache.clone();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let consumer = tokio::spawn(async move {
        run_lookup_consumer(consumer_client, consumer_store, consumer_cache, async {
            shutdown_rx.await.ok();
        })
        .await
        .expect("consumer run");
    });

    tokio::time::sleep(Duration::from_millis(250)).await;

    let request = LookupRequest {
        agent_id: agent_id.clone(),
        tenant_hint: Some("acme".to_string()),
    };
    let direct = lookup(&store, cache.clone(), &request).await.expect("direct lookup");
    assert_eq!(direct, LookupResponse::Found { record: record.clone() });

    let payload = serde_json::to_vec(&request).expect("encode request");
    let response = client
        .request(LOOKUP_SUBJECT.to_string(), payload.into())
        .await
        .expect("nats request");
    let reply: LookupResponse = serde_json::from_slice(&response.payload).expect("decode reply");
    assert_eq!(reply, LookupResponse::Found { record });

    let audit = tokio::time::timeout(Duration::from_secs(5), audit_sub.next())
        .await
        .expect("audit timeout")
        .expect("audit message");
    let audit_body: serde_json::Value = serde_json::from_slice(&audit.payload).expect("audit json");
    assert_eq!(
        audit_body.get("agent_id").and_then(|v| v.as_str()),
        Some(agent_id.as_str())
    );
    assert_eq!(audit_body.get("outcome").and_then(|v| v.as_str()), Some("found"));

    shutdown_tx.send(()).ok();
    consumer.await.expect("consumer task");
}
