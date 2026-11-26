use std::error::Error;
use std::time::{Duration, SystemTime};

use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_auth::{ApiKeyStore, NatsKvApiKeyStore};
use trogon_sandbox_contract::{ServiceAccountId, TenantId};

type TestResult = Result<(), Box<dyn Error>>;

#[tokio::test]
async fn minted_key_resolves_to_its_record_and_is_stored_only_as_a_hash() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let store = NatsKvApiKeyStore::new(server.jetstream().await).await?;
    let before = SystemTime::now() - Duration::from_secs(1);

    let token = store
        .mint(TenantId::new("tenant-a")?, ServiceAccountId::new("service-a"), "ci")
        .await?;

    assert_eq!(token.expose_secret().len(), 64);
    assert!(token.expose_secret().bytes().all(|byte| byte.is_ascii_hexdigit()));
    let record = store.lookup(&token.hash()).await?.ok_or("minted key must resolve")?;
    assert_eq!(record.tenant().as_str(), "tenant-a");
    assert_eq!(record.service_account().as_str(), "service-a");
    assert_eq!(record.name(), "ci");
    assert!(record.created_at() >= before);
    assert!(store.kv().get(token.expose_secret()).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn every_minted_key_is_distinct() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let store = NatsKvApiKeyStore::new(server.jetstream().await).await?;
    let first = store
        .mint(TenantId::new("tenant-a")?, ServiceAccountId::new("service-a"), "one")
        .await?;
    let second = store
        .mint(TenantId::new("tenant-a")?, ServiceAccountId::new("service-a"), "two")
        .await?;
    assert_ne!(first.expose_secret(), second.expose_secret());
    assert_ne!(first.hash(), second.hash());
    Ok(())
}
