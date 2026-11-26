use std::error::Error;
use std::time::Duration;

use async_nats::jetstream::{self, kv};
use futures::StreamExt;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_auth::{
    ApiKeyBucket, ApiKeyRecord, ApiKeyStore, ApiKeyStoreError, BearerToken, DEFAULT_BUCKET, NatsKvApiKeyStore,
};
use trogon_sandbox_contract::{ServiceAccountId, TenantId};

type TestResult = Result<(), Box<dyn Error>>;

const FIXTURE_KEY: &str = "fixture-key";

async fn seed(context: &jetstream::Context, bucket: &str, tenant: &str) -> Result<kv::Store, Box<dyn Error>> {
    let keys = context
        .create_key_value(kv::Config {
            bucket: bucket.to_string(),
            history: 7,
            max_age: Duration::from_secs(300),
            ..Default::default()
        })
        .await?;
    let record = ApiKeyRecord::new(
        TenantId::new(tenant)?,
        ServiceAccountId::new("service-a"),
        "fixture",
        std::time::UNIX_EPOCH,
    );
    keys.put(
        BearerToken::new(FIXTURE_KEY)?.hash().as_str(),
        serde_json::to_vec(&record)?.into(),
    )
    .await?;
    Ok(keys)
}

async fn tenant_of(store: &NatsKvApiKeyStore) -> Result<String, Box<dyn Error>> {
    let record = store
        .lookup(&BearerToken::new(FIXTURE_KEY)?.hash())
        .await?
        .ok_or("the fixture key must exist")?;
    Ok(record.tenant().to_string())
}

#[tokio::test]
async fn fresh_installation_creates_the_canonical_bucket() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    let store = NatsKvApiKeyStore::new(context.clone()).await?;
    assert_eq!(store.kv().status().await?.bucket(), DEFAULT_BUCKET);
    assert_eq!(store.lookup(&BearerToken::new("missing")?.hash()).await?, None);
    Ok(())
}

#[tokio::test]
async fn selected_bucket_is_isolated_and_existing_configuration_is_preserved() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    seed(&context, DEFAULT_BUCKET, "canonical-tenant").await?;
    seed(&context, "custom-api-keys", "custom-tenant").await?;

    for (bucket, tenant) in [
        (DEFAULT_BUCKET, "canonical-tenant"),
        ("custom-api-keys", "custom-tenant"),
    ] {
        let store = NatsKvApiKeyStore::new_with_bucket(context.clone(), &bucket.parse::<ApiKeyBucket>()?).await?;
        assert_eq!(tenant_of(&store).await?, tenant);
        let status = store.kv().status().await?;
        assert_eq!(status.history(), 7);
        assert_eq!(status.max_age(), Duration::from_secs(300));
    }
    let store = NatsKvApiKeyStore::new(context).await?;
    assert_eq!(tenant_of(&store).await?, "canonical-tenant");
    Ok(())
}

#[tokio::test]
async fn unselected_bucket_is_not_created_when_another_exists() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    seed(&context, "custom-api-keys", "custom-tenant").await?;
    let store = NatsKvApiKeyStore::new(context.clone()).await?;
    assert_eq!(store.kv().status().await?.bucket(), DEFAULT_BUCKET);
    assert_eq!(store.lookup(&BearerToken::new(FIXTURE_KEY)?.hash()).await?, None);
    Ok(())
}

#[tokio::test]
async fn malformed_records_are_errors_not_absence() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    let store = NatsKvApiKeyStore::new(context).await?;
    let hash = BearerToken::new("broken")?.hash();
    store.kv().put(hash.as_str(), "not json".into()).await?;
    assert!(matches!(store.lookup(&hash).await, Err(ApiKeyStoreError::Record(_))));

    let hash = BearerToken::new("no-tenant")?.hash();
    store
        .kv()
        .put(
            hash.as_str(),
            r#"{"tenant":"","service_account":"s","name":"n","created_at":1}"#.into(),
        )
        .await?;
    assert!(matches!(store.lookup(&hash).await, Err(ApiKeyStoreError::Record(_))));
    Ok(())
}

#[tokio::test]
async fn refused_bucket_metadata_is_never_interpreted_as_absence() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let mut requests = client.subscribe("AUTH.FIXTURE.>").await?;
    let responder = client.clone();
    let task = tokio::spawn(async move {
        let request = requests.next().await.ok_or("no request")?;
        if request.subject.as_str() != format!("AUTH.FIXTURE.STREAM.INFO.KV_{DEFAULT_BUCKET}") {
            return Err(format!("unexpected subject {}", request.subject).into());
        }
        let reply = request.reply.ok_or("request had no reply subject")?;
        let error = serde_json::json!({"code": 403, "description": "fixture metadata refusal"});
        responder
            .publish(reply, serde_json::to_vec(&serde_json::json!({"error": error}))?.into())
            .await?;
        Ok::<(), Box<dyn Error + Send + Sync>>(())
    });
    let mut context = jetstream::with_prefix(client, "AUTH.FIXTURE");
    context.set_timeout(Duration::from_secs(1));
    let error = NatsKvApiKeyStore::new(context)
        .await
        .err()
        .ok_or("a refused lookup must fail")?;
    assert!(matches!(error, ApiKeyStoreError::Backend(_)));
    assert!(error.to_string().contains("fixture metadata refusal"), "{error}");
    task.await?.map_err(|error| error.to_string())?;
    assert!(server.jetstream().await.get_key_value(DEFAULT_BUCKET).await.is_err());
    Ok(())
}
