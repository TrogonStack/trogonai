use async_nats::jetstream::kv::Config;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::SecretRef;
use trogon_sandbox_providers::{K8sSecretError, resolve_secret_bytes};

const BUCKET: &str = "sandbox-secrets";

fn reference(bucket: &str, key: &str) -> SecretRef {
    SecretRef::NatsKv {
        bucket: bucket.to_string(),
        key: key.to_string(),
    }
}

#[tokio::test]
async fn a_kv_reference_resolves_to_the_stored_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let server = JetStreamTestServer::start().await;
    let jetstream = server.jetstream().await;
    let store = jetstream
        .create_key_value(Config {
            bucket: BUCKET.to_string(),
            ..Default::default()
        })
        .await?;
    store.put("clone", "stored-token".into()).await?;

    let bytes = resolve_secret_bytes(&reference(BUCKET, "clone"), Some(&jetstream)).await?;

    assert_eq!(bytes, b"stored-token");
    Ok(())
}

#[tokio::test]
async fn a_missing_key_is_reported_with_its_bucket() -> Result<(), Box<dyn std::error::Error>> {
    let server = JetStreamTestServer::start().await;
    let jetstream = server.jetstream().await;
    jetstream
        .create_key_value(Config {
            bucket: BUCKET.to_string(),
            ..Default::default()
        })
        .await?;

    let result = resolve_secret_bytes(&reference(BUCKET, "absent"), Some(&jetstream)).await;

    assert!(
        matches!(result, Err(K8sSecretError::NotFound { ref key, ref bucket }) if key == "absent" && bucket == BUCKET)
    );
    Ok(())
}

#[tokio::test]
async fn a_missing_bucket_is_reported() {
    let server = JetStreamTestServer::start().await;
    let jetstream = server.jetstream().await;

    let result = resolve_secret_bytes(&reference("no-such-bucket", "clone"), Some(&jetstream)).await;

    assert!(matches!(result, Err(K8sSecretError::OpenBucket { ref bucket, .. }) if bucket == "no-such-bucket"));
}

#[tokio::test]
async fn a_key_the_store_cannot_read_is_reported() -> Result<(), Box<dyn std::error::Error>> {
    let server = JetStreamTestServer::start().await;
    let jetstream = server.jetstream().await;
    jetstream
        .create_key_value(Config {
            bucket: BUCKET.to_string(),
            ..Default::default()
        })
        .await?;

    let result = resolve_secret_bytes(&reference(BUCKET, "not a valid key"), Some(&jetstream)).await;

    assert!(matches!(result, Err(K8sSecretError::Fetch { .. })));
    Ok(())
}
