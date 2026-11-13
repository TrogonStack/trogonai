use super::*;
use async_nats::jetstream::context::{CreateKeyValueErrorKind, KeyValueErrorKind};
use trogon_credential_domain::{CredentialIdentity, CredentialKind, CredentialOwnerId, CredentialScope, SourceKind};
use trogon_decider_nats::snapshot_key;
use trogon_nats::jetstream::{MockJetStreamKvClient, MockJetStreamKvStore};
use trogonai_proto::gateway::credentials::state_v1;

fn stream_id(owner_id: &str) -> CredentialKey {
    let owner = CredentialOwnerId::new(owner_id).unwrap();
    CredentialIdentity::derive(
        CredentialScope::source(owner, SourceKind::GitHub),
        CredentialKind::WebhookSecret,
    )
    .key()
}

#[test]
fn snapshot_id_uses_nats_safe_deterministic_key() {
    let snapshot_id = credential_snapshot_id(&stream_id("tenant-1"));

    assert_eq!(snapshot_id, credential_snapshot_id(&stream_id("tenant-1")));
    assert!(snapshot_id.starts_with("credential."));
    assert_eq!(snapshot_id.len(), "credential.".len() + 32);
    assert!(
        snapshot_id
            .strip_prefix("credential.")
            .unwrap()
            .chars()
            .all(|ch| ch.is_ascii_hexdigit())
    );
}

#[test]
fn snapshot_id_differs_between_projects_for_the_same_slot() {
    assert_ne!(
        credential_snapshot_id(&stream_id("tenant-1")),
        credential_snapshot_id(&stream_id("tenant-2"))
    );
}

#[test]
fn snapshot_key_keeps_the_project_and_credential_id_out_of_nats_kv_key() {
    let stream_id = stream_id("tenant:1.a");
    let snapshot_id = credential_snapshot_id(&stream_id);
    let key = snapshot_key::<state_v1::CredentialStateSnapshot>(&snapshot_id).unwrap();

    assert!(key.starts_with("snapshots.data.trogonai.gateway.credentials.state.v1.CredentialStateSnapshot."));
    assert!(!key.contains("tenant"));
    assert!(!key.contains(&stream_id.credential_id().to_string()));
    assert!(!key.contains(':'));
    assert!(key.ends_with(&snapshot_id));
}

#[test]
fn snapshot_bucket_config_matches_runtime_contract() {
    let config = snapshot_bucket_config();

    assert_eq!(config.bucket, CREDENTIAL_SNAPSHOT_BUCKET);
    assert_eq!(config.history, 1);
    assert_eq!(config.max_age, Duration::ZERO);
}

#[tokio::test]
async fn provision_snapshot_bucket_uses_create_on_success() {
    let client = MockJetStreamKvClient::new();
    let store = MockJetStreamKvStore::new();
    client.set_create_result(store);

    let result = provision_snapshot_bucket::<_, MockJetStreamKvStore>(&client).await;

    assert!(result.is_ok());
    assert!(client.requested_buckets().is_empty());
    assert_eq!(client.create_configs()[0].bucket, CREDENTIAL_SNAPSHOT_BUCKET);
}

#[tokio::test]
async fn provision_snapshot_bucket_falls_back_to_get_on_already_exists() {
    let client = MockJetStreamKvClient::new();
    client.fail_create_already_exists();
    client.set_get_result(MockJetStreamKvStore::new());

    let result = provision_snapshot_bucket::<_, MockJetStreamKvStore>(&client).await;

    assert!(result.is_ok());
    assert_eq!(client.requested_buckets(), vec![CREDENTIAL_SNAPSHOT_BUCKET.to_string()]);
}

#[tokio::test]
async fn provision_snapshot_bucket_surfaces_get_errors_after_already_exists() {
    let client = MockJetStreamKvClient::new();
    client.fail_create_already_exists();
    client.fail_get(KeyValueErrorKind::GetBucket);

    let error = provision_snapshot_bucket::<_, MockJetStreamKvStore>(&client)
        .await
        .unwrap_err();

    assert!(matches!(error, CredentialEventStoreError::OpenSnapshotBucket(_)));
}

#[tokio::test]
async fn provision_snapshot_bucket_surfaces_create_errors() {
    let client = MockJetStreamKvClient::new();
    client.fail_create(CreateKeyValueErrorKind::TimedOut);

    let error = provision_snapshot_bucket::<_, MockJetStreamKvStore>(&client)
        .await
        .unwrap_err();

    assert!(matches!(error, CredentialEventStoreError::CreateSnapshotBucket(_)));
}

#[tokio::test]
async fn provision_snapshot_bucket_surfaces_status_errors() {
    let client = MockJetStreamKvClient::new();
    let store = MockJetStreamKvStore::new();
    store.fail_status(kv::StatusErrorKind::TimedOut);
    client.set_create_result(store);

    let error = provision_snapshot_bucket::<_, MockJetStreamKvStore>(&client)
        .await
        .unwrap_err();

    assert!(matches!(error, CredentialEventStoreError::InspectSnapshotBucket(_)));
}

#[tokio::test]
async fn provision_snapshot_bucket_rejects_incompatible_history() {
    let client = MockJetStreamKvClient::new();
    let store = MockJetStreamKvStore::new();
    store.set_settings(2, Duration::ZERO, true, None);
    client.set_create_result(store);

    let error = provision_snapshot_bucket::<_, MockJetStreamKvStore>(&client)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        CredentialEventStoreError::IncompatibleSnapshotBucket {
            source: IncompatibleCredentialSnapshotBucketError {
                expected_history: 1,
                actual_history: 2,
                ..
            },
        }
    ));
}

#[tokio::test]
async fn provision_snapshot_bucket_rejects_incompatible_max_age() {
    let client = MockJetStreamKvClient::new();
    let store = MockJetStreamKvStore::new();
    store.set_settings(1, Duration::from_secs(60), true, None);
    client.set_create_result(store);

    let error = provision_snapshot_bucket::<_, MockJetStreamKvStore>(&client)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        CredentialEventStoreError::IncompatibleSnapshotBucket {
            source: IncompatibleCredentialSnapshotBucketError {
                expected_max_age,
                actual_max_age,
                ..
            },
        } if expected_max_age == Duration::ZERO && actual_max_age == Duration::from_secs(60)
    ));
}

mod jetstream_tests;
