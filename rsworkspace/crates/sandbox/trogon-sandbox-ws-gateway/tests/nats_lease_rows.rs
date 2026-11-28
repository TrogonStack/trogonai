mod support;

use std::collections::BTreeMap;

use async_nats::jetstream::kv;
use bytes::Bytes;
use serde_json::{Value, json};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::TenantId;
use trogon_sandbox_ws_gateway::{NatsObservability, ObservabilityConfig};

use support::TestResult;

const LEASES: &str = "lease-row-leases";

fn time(secs: u64) -> Value {
    json!({"secs_since_epoch": secs, "nanos_since_epoch": 0})
}

fn lease(id: &str, tags: Value, spec_extra: Value, record_extra: Value) -> Value {
    let mut spec = json!({
        "image": {"kind": "registry", "reference": {"registry": "ghcr.io", "repository": "acme/app"}},
        "workspace": {"empty": {}},
        "cpu_seconds": 10,
        "memory_mib": 512,
        "env": {},
        "privileged": false,
        "boot_mode": "entrypoint_default",
    });
    let mut record = json!({
        "lease_id": id,
        "tenant": "acme",
        "provider": "mock",
        "status": "running",
        "created_at": time(100),
        "deadline": time(700),
        "tags": tags,
    });
    for (target, extra) in [(&mut spec, spec_extra), (&mut record, record_extra)] {
        if let (Some(target), Value::Object(extra)) = (target.as_object_mut(), extra) {
            target.extend(extra);
        }
    }
    record["spec"] = spec;
    record
}

async fn listed(records: &[(&str, Value)], filter: BTreeMap<String, String>) -> TestResult<Value> {
    let server = JetStreamTestServer::start().await;
    let store = server
        .jetstream()
        .await
        .create_key_value(kv::Config {
            bucket: LEASES.to_string(),
            ..Default::default()
        })
        .await?;
    for (key, record) in records {
        store.put(*key, Bytes::from(serde_json::to_vec(record)?)).await?;
    }
    let observability = NatsObservability::new(
        server.client().await,
        ObservabilityConfig {
            active_lease_bucket: LEASES.to_string(),
        },
    );
    Ok(observability.list_leases(&TenantId::new("acme")?, &filter).await?)
}

#[tokio::test]
async fn a_row_shows_the_idle_policy_and_when_it_paused_the_lease() -> TestResult {
    let record = lease(
        "lease-idle",
        json!({}),
        json!({"idle_policy": {"stages": [{"after_seconds": 60, "action": "pause"}]}}),
        json!({"status": "paused", "last_activity_at": time(400), "auto_paused_at": time(1_700_000_000)}),
    );
    let listing = listed(&[("lease-idle", record)], BTreeMap::new()).await?;
    let row = &listing["leases"][0];
    assert_eq!(row["state"], "paused");
    assert_eq!(row["idle_policy"]["stages"][0]["after_seconds"], 60);
    assert_eq!(row["idle_policy"]["stages"][0]["action"], "pause");
    assert_eq!(row["auto_paused_at"], 1_700_000_000_u64);
    assert_eq!(row["last_activity_at"], 400);
    assert!(row["idle_timeout_seconds"].is_null());
    Ok(())
}

#[tokio::test]
async fn a_row_shows_the_idle_timeout_and_leaves_unset_policy_fields_null() -> TestResult {
    let record = lease(
        "lease-timeout",
        json!({}),
        json!({"idle_timeout_seconds": 300}),
        json!({}),
    );
    let listing = listed(&[("lease-timeout", record)], BTreeMap::new()).await?;
    let row = &listing["leases"][0];
    assert_eq!(row["idle_timeout_seconds"], 300);
    assert!(row["idle_policy"].is_null());
    assert!(row["auto_paused_at"].is_null());
    assert!(row["last_activity_at"].is_null());
    Ok(())
}

#[tokio::test]
async fn a_tag_filter_never_matches_a_lease_without_tags() -> TestResult {
    let untagged = lease("lease-untagged", json!({}), json!({}), json!({}));
    let tagged = lease("lease-tagged", json!({"team": "red"}), json!({}), json!({}));
    let records = [("lease-untagged", untagged), ("lease-tagged", tagged)];

    let filter = BTreeMap::from([("team".to_string(), "red".to_string())]);
    let filtered = listed(&records, filter).await?;
    let rows = filtered["leases"].as_array().ok_or("no leases")?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["lease_id"], "lease-tagged");

    let everything = listed(&records, BTreeMap::new()).await?;
    assert_eq!(everything["leases"].as_array().ok_or("no leases")?.len(), 2);
    Ok(())
}
