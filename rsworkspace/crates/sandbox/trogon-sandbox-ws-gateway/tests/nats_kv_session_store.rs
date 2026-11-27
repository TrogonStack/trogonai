mod support;

use std::time::Duration;

use support::TestResult;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{ServiceAccountId, TenantId};
use trogon_sandbox_ws_gateway::{NatsKvSessionStore, SessionRecord, SessionStore};

fn record(expires_at: u64) -> TestResult<SessionRecord> {
    Ok(SessionRecord {
        tenant: TenantId::new("tenant-test")?,
        service_account: ServiceAccountId::new("service-account"),
        api_key: "stored-key".to_string(),
        expires_at,
    })
}

#[tokio::test]
async fn sessions_round_trip_and_expired_ones_are_dropped() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let store = NatsKvSessionStore::new(server.jetstream().await, Duration::from_secs(60)).await?;
    assert!(store.lookup("missing").await?.is_none());

    store.put("live", record(u64::MAX)?).await?;
    let found = store.lookup("live").await?.ok_or("the live session is gone")?;
    assert_eq!(found.tenant, TenantId::new("tenant-test")?);
    assert_eq!(found.api_key, "stored-key");

    store.put("stale", record(1)?).await?;
    assert!(store.lookup("stale").await?.is_none());
    assert!(store.lookup("stale").await?.is_none(), "an expired session is deleted");
    Ok(())
}
