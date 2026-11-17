use std::time::Duration;

use async_nats::jetstream::context::CreateStreamErrorKind;
use async_nats::jetstream::stream::Config;
use trogon_nats::test_support::JetStreamTestServer;

use super::*;
use crate::credential::constants::CREDENTIAL_EVENT_SUBJECT_PREFIX;
use crate::credential::event_stream_mismatch_error::CredentialEventStreamMismatchError;

/// What `STREAM.CREATE` answers when the name is taken by another configuration.
fn name_in_use() -> CreateStreamError {
    let source: async_nats::jetstream::Error = serde_json::from_str(
        r#"{"code":400,"err_code":10058,"description":"stream name already in use with a different configuration"}"#,
    )
    .unwrap();
    CreateStreamError::new(CreateStreamErrorKind::JetStream(source))
}

#[tokio::test]
async fn creates_the_stream_with_the_credential_configuration() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;

    let created = ensure_credential_event_stream(&js).await.unwrap();

    let config = js
        .get_stream(CREDENTIAL_STREAM)
        .await
        .unwrap()
        .cached_info()
        .config
        .clone();
    assert_eq!(created.cached_info().config.name, CREDENTIAL_STREAM);
    assert_eq!(config.subjects, vec![format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.>")]);
    assert!(config.max_age.is_zero());
    assert!(config.max_messages_per_subject <= 0);
    assert!(config.max_messages <= 0);
    assert!(config.max_bytes <= 0);
}

#[tokio::test]
async fn ensuring_twice_changes_nothing() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    ensure_credential_event_stream(&js).await.unwrap();
    let before = js.get_stream(CREDENTIAL_STREAM).await.unwrap().cached_info().created;

    let again = ensure_credential_event_stream(&js).await.unwrap();

    assert_eq!(again.cached_info().created, before);
    assert_eq!(again.cached_info().state.messages, 0);
}

#[tokio::test]
async fn concurrent_ensures_both_succeed() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;

    let (first, second) = tokio::join!(ensure_credential_event_stream(&js), ensure_credential_event_stream(&js),);

    assert_eq!(first.unwrap().cached_info().config.name, CREDENTIAL_STREAM);
    assert_eq!(second.unwrap().cached_info().config.name, CREDENTIAL_STREAM);
}

#[tokio::test]
async fn refuses_a_stream_that_could_drop_events_and_leaves_it_as_it_found_it() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    type Weaken = fn(&mut Config);
    let cases: [(Weaken, CredentialEventStreamMismatchError); 4] = [
        (
            |c| c.max_age = Duration::from_secs(3600),
            CredentialEventStreamMismatchError::ExpiresByAge,
        ),
        (
            |c| c.max_messages_per_subject = 10,
            CredentialEventStreamMismatchError::CapsEventsPerSubject,
        ),
        (|c| c.max_messages = 10, CredentialEventStreamMismatchError::CapsEvents),
        (
            |c| c.max_bytes = 1024 * 1024,
            CredentialEventStreamMismatchError::CapsBytes,
        ),
    ];
    for (weaken, expected) in cases {
        let mut weakened = stream_config();
        weaken(&mut weakened);
        js.create_stream(weakened).await.unwrap();

        let error = ensure_credential_event_stream(&js).await.unwrap_err();

        assert!(
            matches!(&error, EnsureCredentialEventStreamError::ConfigMismatch(found) if *found == expected),
            "expected {expected:?}, found {error:?}"
        );
        let kept = js
            .get_stream(CREDENTIAL_STREAM)
            .await
            .unwrap()
            .cached_info()
            .config
            .clone();
        assert_eq!(
            validate_stream_config(&kept),
            Err(expected),
            "the weakened limit was rewritten"
        );
        js.delete_stream(CREDENTIAL_STREAM).await.unwrap();
    }
}

#[tokio::test]
async fn a_create_the_server_refuses_is_a_create_error() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    js.create_stream(Config {
        name: "CREDENTIAL_EVENTS_SQUATTER".to_string(),
        subjects: vec![format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.>")],
        ..Default::default()
    })
    .await
    .unwrap();

    let error = ensure_credential_event_stream(&js).await.unwrap_err();

    assert!(matches!(error, EnsureCredentialEventStreamError::Create { .. }));
    assert!(js.get_stream(CREDENTIAL_STREAM).await.is_err());
}

#[tokio::test]
async fn a_lookup_that_fails_for_another_reason_than_absence_is_a_get_error() {
    let server = JetStreamTestServer::start().await;
    let js = async_nats::jetstream::with_prefix(server.client().await, "no.such.api");

    let error = ensure_credential_event_stream(&js).await.unwrap_err();

    assert!(matches!(error, EnsureCredentialEventStreamError::Get { .. }));
}

#[tokio::test]
async fn a_create_that_lost_the_race_validates_the_stream_that_won() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    ensure_credential_event_stream(&js).await.unwrap();

    let settled = settle_create(&js, Err(name_in_use())).await;

    assert_eq!(settled.unwrap().cached_info().config.name, CREDENTIAL_STREAM);
}

#[tokio::test]
async fn a_create_that_lost_the_race_to_a_weakened_stream_is_refused() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let mut weakened = stream_config();
    weakened.max_age = Duration::from_secs(3600);
    js.create_stream(weakened).await.unwrap();

    let error = settle_create(&js, Err(name_in_use())).await.unwrap_err();

    assert!(matches!(
        error,
        EnsureCredentialEventStreamError::ConfigMismatch(CredentialEventStreamMismatchError::ExpiresByAge)
    ));
}

#[tokio::test]
async fn a_conflict_over_a_stream_that_cannot_be_read_back_is_a_get_error() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;

    let error = settle_create(&js, Err(name_in_use())).await.unwrap_err();

    assert!(matches!(error, EnsureCredentialEventStreamError::Get { .. }));
}

#[test]
fn errors_name_the_stream_and_the_limit() {
    assert_eq!(
        EnsureCredentialEventStreamError::ConfigMismatch(CredentialEventStreamMismatchError::CapsBytes).to_string(),
        "stream 'CREDENTIAL_EVENTS' must not cap the bytes it keeps"
    );
    assert_eq!(
        EnsureCredentialEventStreamError::Create { source: name_in_use() }.to_string(),
        "failed to create stream 'CREDENTIAL_EVENTS'"
    );
}
