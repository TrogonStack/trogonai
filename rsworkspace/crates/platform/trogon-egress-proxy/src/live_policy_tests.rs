use std::num::NonZeroU64;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::AgentId;
use crate::destination::Destination;
use crate::live_policy::{KvPolicyConfig, KvSnapshot};
use crate::policy_snapshot::{PolicySnapshot, SnapshotVerdict};
use async_nats::jetstream::kv;
use tokio_util::sync::CancellationToken;
use trogon_audit::AuditName;
use trogon_nats::NatsToken;
use trogon_nats::test_support::JetStreamTestServer;

use crate::constants::{LIVE_POLICY_TEST_POLL as POLL, LIVE_POLICY_TEST_WAIT as WAIT};

fn envelope(generation: u64, agent: &str) -> Vec<u8> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    serde_json::to_vec(&serde_json::json!({
        "tenant": "acme",
        "project": "payments",
        "incarnation": 1,
        "generation": generation,
        "issued_at": now,
        "valid_until": now + 30,
        "policy": format!("[agents.{agent}]\nallow = [\"api.example.com:443\"]\n")
    }))
    .unwrap()
}

#[tokio::test]
async fn a_complete_live_snapshot_replaces_the_previously_permitted_agent() {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let store = async_nats::jetstream::new(client.clone())
        .create_key_value(kv::Config {
            bucket: "egress_policy".into(),
            ..kv::Config::default()
        })
        .await
        .unwrap();
    store.put("current", envelope(1, "agent-1").into()).await.unwrap();
    let (snapshot, driver) = KvSnapshot::open(
        client,
        KvPolicyConfig {
            bucket: "egress_policy".into(),
            tenant: AuditName::new("acme").unwrap(),
            project: NatsToken::new("payments").unwrap(),
            incarnation: NonZeroU64::new(1).unwrap(),
        },
    )
    .await
    .unwrap();
    let agent = AgentId::new("agent-1").unwrap();
    let destination: Destination = "api.example.com:443".parse().unwrap();
    assert_eq!(
        snapshot.lookup(&agent, &destination, SystemTime::now()),
        SnapshotVerdict::Permitted
    );
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    store.put("current", envelope(2, "agent-2").into()).await.unwrap();
    let changed = tokio::time::timeout(WAIT, async {
        while snapshot.lookup(&agent, &destination, SystemTime::now()) != SnapshotVerdict::NotPermitted {
            tokio::time::sleep(POLL).await;
        }
    })
    .await;
    shutdown.cancel();
    task.await.unwrap();
    assert!(changed.is_ok(), "a complete replacement must remove the prior permit");
    assert_eq!(
        snapshot.lookup(&AgentId::new("agent-2").unwrap(), &destination, SystemTime::now()),
        SnapshotVerdict::Permitted
    );
}
