use std::num::NonZeroU64;
use std::time::{SystemTime, UNIX_EPOCH};

use async_nats::jetstream::kv;
use trogon_audit::AuditName;
use trogon_nats::NatsToken;
use trogon_nats::test_support::JetStreamTestServer;

use super::*;

#[tokio::test]
async fn rejecting_a_candidate_withdraws_before_the_state_lock_is_released() {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let config = KvPolicyConfig::new(
        "egress_policy",
        AuditName::new("acme").unwrap(),
        NatsToken::new("payments").unwrap(),
        NonZeroU64::new(1).unwrap(),
    )
    .unwrap();
    let store = async_nats::jetstream::new(client.clone())
        .create_key_value(kv::Config {
            bucket: config.bucket().to_owned(),
            max_value_size: LIVE_POLICY_MAX_BYTES as i32,
            max_bytes: LIVE_POLICY_MAX_STORE_BYTES,
            max_age: LIVE_POLICY_MAX_LIFETIME,
            ..kv::Config::default()
        })
        .await
        .unwrap();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let mut document = serde_json::json!({"tenant":"acme", "project":"payments", "incarnation":1, "generation":1, "issued_at":now, "valid_until":now+30, "policy":"[agents.agent-1]\nallow=[\"api.example.com:443\"]\n"});
    store
        .put(config.key(), serde_json::to_vec(&document).unwrap().into())
        .await
        .unwrap();
    let (snapshot, _driver) = KvSnapshot::open(client, config.clone()).await.unwrap();
    document["tenant"] = serde_json::json!("another");
    document["generation"] = serde_json::json!(2);
    store
        .put(config.key(), serde_json::to_vec(&document).unwrap().into())
        .await
        .unwrap();
    let candidate = store
        .stream
        .get_last_raw_message_by_subject(&format!("$KV.{}.{}", config.bucket(), config.key()))
        .await
        .unwrap();
    assert!(matches!(snapshot.apply(candidate), Err(LivePolicyError::Scope)));
    assert_eq!(
        snapshot.lookup(
            &AgentId::new("agent-1").unwrap(),
            &"api.example.com:443".parse().unwrap(),
            SystemTime::now()
        ),
        SnapshotVerdict::Unavailable
    );
}
