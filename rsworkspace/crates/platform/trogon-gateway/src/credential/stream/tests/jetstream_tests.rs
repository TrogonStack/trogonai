use trogon_credential_domain::{CredentialIdentity, CredentialKind, CredentialScope, SourceKind};
use trogon_decider_nats::{StreamSubjectResolver, SubjectState};
use trogon_decider_runtime::{
    AppendStreamRequest, Event, EventId, Headers, StreamAppend, StreamPosition, StreamWritePrecondition,
};
use trogon_nats::test_support::JetStreamTestServer;
use uuid::Uuid;

use super::*;
use crate::credential::store;

fn stream_id(owner_id: &str) -> CredentialKey {
    let owner = CredentialOwnerId::new(owner_id).unwrap();
    CredentialIdentity::derive(
        CredentialScope::source(owner, SourceKind::GitHub),
        CredentialKind::WebhookSecret,
    )
    .key()
}

async fn resolve(
    store: &store::CredentialEventStore,
    stream_id: &CredentialKey,
) -> Result<SubjectState, CredentialEventSubjectResolverError> {
    CredentialEventSubjectResolver::for_owner(stream_id.owner_id())?
        .resolve_subject_state(store.events_stream(), stream_id)
        .await
}

#[tokio::test]
async fn resolver_reports_the_event_subject_without_a_position_for_a_new_credential() {
    let server = JetStreamTestServer::start().await;
    let store = store::open(server.jetstream().await).await.unwrap();
    let stream_id = stream_id("tenant-1");

    let state = resolve(&store, &stream_id).await.unwrap();

    assert_eq!(
        state.subject.as_str(),
        credential_event_subject(&stream_id).unwrap().as_str()
    );
    assert_eq!(state.current_position, None);
}

#[tokio::test]
async fn resolver_reports_the_latest_position_of_the_credential_subject() {
    let server = JetStreamTestServer::start().await;
    let store = store::open(server.jetstream().await).await.unwrap();
    let events = [1u128, 2]
        .into_iter()
        .map(|id| Event {
            id: EventId::from(Uuid::from_u128(id)),
            r#type: "test.event.v1".to_string(),
            content: b"payload".to_vec(),
            headers: Headers::empty(),
        })
        .collect();
    store
        .append_stream(AppendStreamRequest {
            stream_id: &stream_id("tenant-1"),
            stream_write_precondition: StreamWritePrecondition::NoStream,
            events,
        })
        .await
        .unwrap();

    let state = resolve(&store, &stream_id("tenant-1")).await.unwrap();
    let other = resolve(&store, &stream_id("tenant-2")).await.unwrap();

    assert_eq!(state.current_position, Some(StreamPosition::try_new(2).unwrap()));
    assert_eq!(other.current_position, None);
}

#[tokio::test]
async fn resolver_surfaces_stream_errors_when_the_stream_is_gone() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let store = store::open(js.clone()).await.unwrap();
    js.delete_stream(CREDENTIAL_STREAM).await.unwrap();

    let error = resolve(&store, &stream_id("tenant-1")).await.unwrap_err();

    assert!(matches!(error, CredentialEventSubjectResolverError::Stream(_)));
}
