use trogon_decider_runtime::{Event, EventId, Headers, ReadFrom, Snapshot, StreamPosition, StreamWritePrecondition};
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
