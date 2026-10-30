use super::*;
use async_nats::jetstream::context::{CreateKeyValueErrorKind, KeyValueErrorKind};
use trogon_decider_nats::snapshot_key;
use trogon_nats::jetstream::{MockJetStreamKvClient, MockJetStreamKvStore};
use trogonai_proto::gateway::credentials::state_v1;

#[test]
fn snapshot_id_uses_nats_safe_deterministic_key() {
    let snapshot_id = credential_snapshot_id("openbao:tenant-1:github/primary:webhook_secret");

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
fn snapshot_key_keeps_raw_credential_id_out_of_nats_kv_key() {
    let raw_stream_id = "openbao:tenant-1:github/primary:webhook_secret";
    let snapshot_id = credential_snapshot_id(raw_stream_id);
    let key = snapshot_key::<state_v1::CredentialStateSnapshot>(&snapshot_id).unwrap();

    assert!(key.starts_with("snapshots.data.trogonai.gateway.credentials.state.v1.CredentialStateSnapshot."));
    assert!(!key.contains(raw_stream_id));
    assert!(!key.contains(':'));
    assert!(!key.contains('/'));
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

mod jetstream {
    use trogon_decider_runtime::{
        Event, EventId, Headers, ReadFrom, Snapshot, StreamPosition, StreamWritePrecondition,
    };
    use trogon_nats::test_support::JetStreamTestServer;
    use uuid::Uuid;

    use super::*;

    const STREAM_ID: &str = "openbao:tenant-1:github/primary:webhook_secret";

    fn event(id: u128, content: &[u8]) -> Event {
        Event {
            id: EventId::from(Uuid::from_u128(id)),
            r#type: "test.event.v1".to_string(),
            content: content.to_vec(),
            headers: Headers::empty(),
        }
    }

    async fn open_store(server: &JetStreamTestServer) -> CredentialEventStore {
        open(server.jetstream().await).await.unwrap()
    }

    #[tokio::test]
    async fn open_provisions_the_event_stream_and_exposes_its_handles() {
        let server = JetStreamTestServer::start().await;

        let store = open_store(&server).await;

        assert_eq!(
            store.events_stream().cached_info().config.name,
            crate::credential::constants::CREDENTIAL_STREAM
        );
        let mut stream = store
            .as_jetstream()
            .get_stream(crate::credential::constants::CREDENTIAL_STREAM)
            .await
            .unwrap();
        assert_eq!(stream.info().await.unwrap().state.messages, 0);
    }

    #[tokio::test]
    async fn open_is_idempotent_over_existing_resources() {
        let server = JetStreamTestServer::start().await;

        open_store(&server).await;
        let second = open(server.jetstream().await).await;

        assert!(second.is_ok());
    }

    #[tokio::test]
    async fn appended_events_are_read_back_in_order() {
        let server = JetStreamTestServer::start().await;
        let store = open_store(&server).await;

        let appended = store
            .append_stream(AppendStreamRequest {
                stream_id: STREAM_ID,
                stream_write_precondition: StreamWritePrecondition::NoStream,
                events: vec![event(1, b"first"), event(2, b"second")],
            })
            .await
            .unwrap();
        let read = store
            .read_stream(ReadStreamRequest {
                stream_id: STREAM_ID,
                from: ReadFrom::Beginning,
            })
            .await
            .unwrap();

        assert_eq!(read.current_position, Some(appended.stream_position));
        let contents: Vec<&[u8]> = read.events.iter().map(|event| event.event.content.as_slice()).collect();
        assert_eq!(contents, [b"first".as_slice(), b"second".as_slice()]);
        assert!(read.events.iter().all(|event| event.stream_id() == STREAM_ID));
    }

    #[tokio::test]
    async fn streams_for_different_credentials_do_not_see_each_other() {
        let server = JetStreamTestServer::start().await;
        let store = open_store(&server).await;
        store
            .append_stream(AppendStreamRequest {
                stream_id: STREAM_ID,
                stream_write_precondition: StreamWritePrecondition::Any,
                events: vec![event(1, b"mine")],
            })
            .await
            .unwrap();

        let other = store
            .read_stream(ReadStreamRequest {
                stream_id: "openbao:tenant-2:github/primary:webhook_secret",
                from: ReadFrom::Beginning,
            })
            .await
            .unwrap();

        assert!(other.events.is_empty());
        assert_eq!(other.current_position, None);
    }

    #[tokio::test]
    async fn snapshot_round_trips_under_the_hashed_credential_key() {
        let server = JetStreamTestServer::start().await;
        let store = open_store(&server).await;
        let snapshot = Snapshot::new(
            StreamPosition::try_new(7).unwrap(),
            state_v1::CredentialStateSnapshot::default(),
        );

        let missing: ReadSnapshotResponse<state_v1::CredentialStateSnapshot> = store
            .read_snapshot(ReadSnapshotRequest { snapshot_id: STREAM_ID })
            .await
            .unwrap();
        store
            .write_snapshot(WriteSnapshotRequest {
                snapshot_id: STREAM_ID,
                snapshot: snapshot.clone(),
            })
            .await
            .unwrap();
        let loaded: ReadSnapshotResponse<state_v1::CredentialStateSnapshot> = store
            .read_snapshot(ReadSnapshotRequest { snapshot_id: STREAM_ID })
            .await
            .unwrap();

        assert!(missing.snapshot.is_none());
        assert_eq!(loaded.snapshot, Some(snapshot));
    }
}
