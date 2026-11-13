//! The read against a real JetStream server, so the bound, the subject filter,
//! and the leader reads are those of NATS rather than of a mock.

use async_nats::jetstream::{self, stream};
use trogon_credential_aggregate::{CREDENTIAL_EVENT_SUBJECT_PREFIX, CREDENTIAL_STREAM};
use trogon_nats::test_support::JetStreamTestServer;

use super::*;

async fn credential_stream(js: &jetstream::Context) -> stream::Stream {
    js.create_stream(stream::Config {
        name: CREDENTIAL_STREAM.to_owned(),
        subjects: vec![format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.>")],
        ..Default::default()
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn reads_credential_states_from_a_jetstream_credential_stream() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let events = credential_stream(&js).await;
    let states = JetStreamCredentialStates::new(events.clone());
    let identity = identity();
    let neighbour = neighbour();

    assert_eq!(states.read(&identity.key()).await.unwrap(), None, "unknown credential");

    append(
        &js,
        [
            message(&identity, &write_requested(&identity)),
            message(&neighbour, &write_requested(&neighbour)),
            message(&identity, &activated(&identity)),
        ],
    )
    .await;
    let active = states.read(&identity.key()).await.unwrap().unwrap();
    assert!(
        matches!(state_case(&active), CredentialStateSnapshotCase::Active(_)),
        "{active:?}"
    );

    append(&js, [message(&identity, &revoked(&identity))]).await;
    let revoked_state = states.read(&identity.key()).await.unwrap().unwrap();
    assert!(
        matches!(state_case(&revoked_state), CredentialStateSnapshotCase::Revoked(_)),
        "{revoked_state:?}"
    );

    append(
        &js,
        [
            message(&identity, &destroy_requested(&identity)),
            message(&identity, &destroyed(&identity)),
        ],
    )
    .await;
    let destroyed_state = states.read(&identity.key()).await.unwrap().unwrap();
    assert!(
        matches!(state_case(&destroyed_state), CredentialStateSnapshotCase::Destroyed(_)),
        "{destroyed_state:?}"
    );

    let pending = states.read(&neighbour.key()).await.unwrap().unwrap();
    assert!(
        matches!(state_case(&pending), CredentialStateSnapshotCase::PendingWrite(_)),
        "{pending:?}"
    );
}

#[tokio::test]
async fn fails_closed_on_a_corrupt_event_in_a_jetstream_credential_stream() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let events = credential_stream(&js).await;
    let identity = identity();
    append(
        &js,
        [
            message(&identity, &write_requested(&identity)),
            message(&identity, &activated(&identity)),
            raw_message(
                &identity,
                Some(revoked(&identity).event_type().unwrap()),
                vec![0xff, 0xff],
            ),
        ],
    )
    .await;

    let error = JetStreamCredentialStates::new(events)
        .read(&identity.key())
        .await
        .unwrap_err();

    assert!(
        matches!(error, CredentialStateReadError::Undecodable { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn leaves_events_appended_after_the_bound_of_a_jetstream_read_unread() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let events = credential_stream(&js).await;
    let identity = identity();
    append(
        &js,
        [
            message(&identity, &write_requested(&identity)),
            message(&identity, &activated(&identity)),
        ],
    )
    .await;
    let growing = GrowingStream::new(events.clone(), js.clone(), message(&identity, &revoked(&identity)));

    let during = JetStreamCredentialStates::new(growing)
        .read(&identity.key())
        .await
        .unwrap()
        .unwrap();
    let after = JetStreamCredentialStates::new(events)
        .read(&identity.key())
        .await
        .unwrap()
        .unwrap();

    assert!(
        matches!(state_case(&during), CredentialStateSnapshotCase::Active(_)),
        "{during:?}"
    );
    assert!(
        matches!(state_case(&after), CredentialStateSnapshotCase::Revoked(_)),
        "{after:?}"
    );
}
