use super::*;
use crate::jetstream::{MockJetStreamKvClient, MockJetStreamKvStore};
use crate::lease::{LeaseRenewInterval, LeaseTtl};
use async_nats::jetstream::context::CreateKeyValueErrorKind;

fn ttl(secs: u64) -> LeaseTtl {
    LeaseTtl::from_secs(secs).unwrap()
}

fn renew_interval(secs: u64) -> LeaseRenewInterval {
    LeaseRenewInterval::from_secs(secs).unwrap()
}

fn config() -> NatsKvLeaseConfig {
    NatsKvLeaseConfig::new("bucket", "key", ttl(10), renew_interval(5)).unwrap()
}

fn strict_settings(ttl: Duration) -> KeyValueSettings {
    KeyValueSettings {
        history: 1,
        max_age: Duration::ZERO,
        allow_message_ttl: true,
        subject_delete_marker_ttl: Some(ttl),
    }
}

fn apply_settings(store: &MockJetStreamKvStore, settings: KeyValueSettings) {
    store.set_settings(
        settings.history,
        settings.max_age,
        settings.allow_message_ttl,
        settings.subject_delete_marker_ttl,
    );
}

#[test]
fn key_value_config_sets_expected_values() {
    let config = config();
    let kv_config = key_value_config(&config);

    assert_eq!(kv_config.bucket, "bucket");
    assert_eq!(kv_config.history, 1);
    assert_eq!(kv_config.max_age, Duration::ZERO);
    assert_eq!(kv_config.limit_markers, Some(Duration::from_secs(10)));
}

#[tokio::test]
async fn provision_store_uses_create_on_success() {
    let config = config();
    let store = MockJetStreamKvStore::new();
    apply_settings(&store, strict_settings(config.ttl()));

    let client = MockJetStreamKvClient::new();
    client.set_create_result(store);

    let result = provision_store(&client, &config).await;
    assert!(result.is_ok());
    assert!(client.requested_buckets().is_empty());
    assert_eq!(client.create_configs()[0].bucket, "bucket");
}

#[tokio::test]
async fn provision_store_falls_back_to_get_on_already_exists() {
    let config = config();
    let store = MockJetStreamKvStore::new();
    apply_settings(&store, strict_settings(config.ttl()));

    let client = MockJetStreamKvClient::new();
    client.fail_create_already_exists();
    client.set_get_result(store);

    let result = provision_store(&client, &config).await;
    assert!(result.is_ok());
    assert_eq!(client.requested_buckets(), vec!["bucket".to_string()]);
}

#[tokio::test]
async fn provision_store_surfaces_get_errors_after_already_exists() {
    let config = config();
    let client = MockJetStreamKvClient::new();
    client.fail_create_already_exists();
    client.fail_get(async_nats::jetstream::context::KeyValueErrorKind::GetBucket);

    let error = provision_store::<_, MockJetStreamKvStore>(&client, &config)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "lease provision error: failed to open existing lease bucket after create reported already exists: failed to get the bucket"
    );
}

#[tokio::test]
async fn provision_store_surfaces_create_errors() {
    let config = config();
    let client = MockJetStreamKvClient::new();
    client.fail_create(CreateKeyValueErrorKind::TimedOut);

    let error = provision_store::<_, MockJetStreamKvStore>(&client, &config)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "lease provision error: failed to create lease bucket: timed out"
    );
}

#[tokio::test]
async fn provision_store_surfaces_status_errors() {
    let config = config();
    let store = MockJetStreamKvStore::new();
    store.fail_status(kv::StatusErrorKind::TimedOut);

    let client = MockJetStreamKvClient::new();
    client.set_create_result(store);

    let error = provision_store::<_, MockJetStreamKvStore>(&client, &config)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "lease provision error: failed to inspect lease bucket configuration: timed out"
    );
}

#[tokio::test]
async fn provision_store_rejects_incompatible_bucket_settings() {
    let config = config();
    let store = MockJetStreamKvStore::new();
    apply_settings(
        &store,
        KeyValueSettings {
            history: 2,
            ..strict_settings(config.ttl())
        },
    );

    let client = MockJetStreamKvClient::new();
    client.set_create_result(store);

    let error = provision_store::<_, MockJetStreamKvStore>(&client, &config)
        .await
        .unwrap_err();
    assert!(matches!(error, LeaseError::IncompatibleBucketConfig { .. }));
}
