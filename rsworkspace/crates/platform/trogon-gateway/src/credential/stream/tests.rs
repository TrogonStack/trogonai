use super::*;

fn credential_id(raw: &str) -> CredentialId {
    CredentialId::new(raw).unwrap()
}

#[test]
fn key_derivation_is_deterministic_for_the_same_credential_id() {
    let first = CredentialKey::derive(&credential_id("openbao:tenant-1:github/primary:webhook_secret"));
    let second = CredentialKey::derive(&credential_id("openbao:tenant-1:github/primary:webhook_secret"));

    assert_eq!(first, second);
}

#[test]
fn key_derivation_keeps_nats_unsafe_ids_out_of_subject_tokens() {
    let key = CredentialKey::derive(&credential_id("openbao:tenant-1:github/primary:webhook_secret"));
    let subject = CredentialSubject::event(&key).unwrap();

    assert_eq!(
        subject.as_str(),
        format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.{}", key.simple())
    );
    assert_eq!(subject.key(), &key);
    assert_eq!(key.simple().len(), 32);
    assert!(key.simple().chars().all(|ch| ch.is_ascii_hexdigit()));
}

#[test]
fn stream_id_routing_preserves_key_derivation() {
    let raw = "openbao:tenant-1:github/primary:webhook_secret";
    let stream_id = CredentialStreamId::from(raw);

    assert_eq!(
        CredentialKey::for_stream(&stream_id),
        CredentialKey::derive(&credential_id(raw))
    );
}

#[test]
fn stream_config_matches_credential_contract() {
    let config = stream_config();

    assert_eq!(config.name, CREDENTIAL_STREAM);
    assert_eq!(config.subjects, vec![format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.>")]);
    assert!(config.allow_atomic_publish);
}

mod jetstream {
    use trogon_decider_nats::{StreamSubjectResolver, SubjectState};
    use trogon_decider_runtime::{
        AppendStreamRequest, Event, EventId, Headers, StreamAppend, StreamPosition, StreamWritePrecondition,
    };
    use trogon_nats::test_support::JetStreamTestServer;
    use uuid::Uuid;

    use super::*;
    use crate::credential::store;

    const STREAM_ID: &str = "openbao:tenant-1:github/primary:webhook_secret";

    async fn resolve(
        store: &store::CredentialEventStore,
        stream_id: &str,
    ) -> Result<SubjectState, CredentialEventSubjectResolverError> {
        CredentialEventSubjectResolver
            .resolve_subject_state(store.events_stream(), stream_id)
            .await
    }

    #[tokio::test]
    async fn resolver_reports_the_event_subject_without_a_position_for_a_new_credential() {
        let server = JetStreamTestServer::start().await;
        let store = store::open(server.jetstream().await).await.unwrap();

        let state = resolve(&store, STREAM_ID).await.unwrap();

        let key = CredentialKey::for_stream(&CredentialStreamId::from(STREAM_ID));
        assert_eq!(state.subject.as_str(), CredentialSubject::event(&key).unwrap().as_str());
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
                stream_id: STREAM_ID,
                stream_write_precondition: StreamWritePrecondition::NoStream,
                events,
            })
            .await
            .unwrap();

        let state = resolve(&store, STREAM_ID).await.unwrap();
        let other = resolve(&store, "openbao:tenant-2:github/primary:webhook_secret")
            .await
            .unwrap();

        assert_eq!(state.current_position, Some(StreamPosition::try_new(2).unwrap()));
        assert_eq!(other.current_position, None);
    }

    #[tokio::test]
    async fn resolver_surfaces_stream_errors_when_the_stream_is_gone() {
        let server = JetStreamTestServer::start().await;
        let js = server.jetstream().await;
        let store = store::open(js.clone()).await.unwrap();
        js.delete_stream(CREDENTIAL_STREAM).await.unwrap();

        let error = resolve(&store, STREAM_ID).await.unwrap_err();

        assert!(matches!(error, CredentialEventSubjectResolverError::Stream(_)));
    }
}
