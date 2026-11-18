#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::Duration;

use async_nats::jetstream;
use trogon_decider_nats::{JetStreamStore, JetStreamStoreError};
use trogon_decider_runtime::{
    AppendStreamRequest, CommandId, Event, Headers, ReadFrom, ReadStreamRequest, StreamAppend, StreamRead,
    StreamWritePrecondition,
};
use trogon_nats::{NatsToken, test_support::JetStreamTestServer};
use trogon_session_store::{SessionKey, SessionNativeError, SessionStreamProfile, SessionSubjectResolver};

async fn server() -> JetStreamTestServer {
    match std::env::var("TROGON_SESSION_TEST_NETWORK") {
        Ok(network) => JetStreamTestServer::start_on_network(&network).await,
        Err(_) => JetStreamTestServer::start().await,
    }
}

fn config(name: &str, namespace: &str) -> jetstream::stream::Config {
    jetstream::stream::Config {
        name: name.into(),
        subjects: vec![format!("{namespace}.session.sessions.>")],
        storage: jetstream::stream::StorageType::File,
        retention: jetstream::stream::RetentionPolicy::Limits,
        discard: jetstream::stream::DiscardPolicy::New,
        max_bytes: 4 * 1024 * 1024,
        max_messages: 1024,
        max_message_size: 262144,
        deny_delete: true,
        deny_purge: true,
        allow_atomic_publish: true,
        num_replicas: 1,
        ..Default::default()
    }
}

async fn store(
    js: &jetstream::Context,
    config: jetstream::stream::Config,
    profile: SessionStreamProfile,
) -> JetStreamStore<SessionSubjectResolver> {
    let stream = js.create_stream(config).await.expect("create Session stream");
    let snapshots = js
        .create_key_value(jetstream::kv::Config {
            bucket: format!("SESSION_NATIVE_PROFILE_{}", uuid::Uuid::new_v4().simple()),
            ..Default::default()
        })
        .await
        .expect("create snapshots");
    JetStreamStore::builder(js.clone(), stream, snapshots).with_subject_resolver(SessionSubjectResolver::new(profile))
}

fn append(key: &SessionKey) -> AppendStreamRequest<'_, SessionKey> {
    AppendStreamRequest {
        stream_id: key,
        stream_write_precondition: StreamWritePrecondition::NoStream,
        events: vec![Event {
            id: CommandId::new(uuid::Uuid::new_v4()).event_id(0),
            r#type: "fixture.session.event".into(),
            content: b"fixture occurrence".to_vec(),
            headers: Headers::empty(),
        }],
    }
}

#[tokio::test]
async fn current_retention_profile_is_rechecked_before_each_native_read() {
    let server = server().await;
    let js = server.jetstream().await;
    let config = config("SESSION_NATIVE_PROFILE", "fixture");
    let namespace = NatsToken::new("fixture").unwrap();
    let profile = SessionStreamProfile::new(config.name.clone(), namespace.clone(), 1).unwrap();
    let store = store(&js, config.clone(), profile).await;
    let key = SessionKey::new(namespace, "parent").unwrap();
    assert!(
        store
            .read_stream(ReadStreamRequest {
                stream_id: &key,
                from: ReadFrom::Beginning
            })
            .await
            .expect("valid profile read")
            .events
            .is_empty()
    );

    js.update_stream(jetstream::stream::Config {
        max_age: Duration::from_secs(3600),
        ..config
    })
    .await
    .expect("withdraw retention guarantee");
    let result = store
        .read_stream(ReadStreamRequest {
            stream_id: &key,
            from: ReadFrom::Beginning,
        })
        .await;
    assert!(
        matches!(
            result,
            Err(JetStreamStoreError::ResolveSubject(SessionNativeError::ProfileMismatch))
        ),
        "a current retention violation must refuse the native read before replay: {result:?}"
    );
    assert!(matches!(
        store.append_stream(append(&key)).await,
        Err(JetStreamStoreError::ResolveSubject(SessionNativeError::ProfileMismatch))
    ));
    assert_eq!(store.events_stream().get_info().await.unwrap().state.messages, 0);
}

#[tokio::test]
async fn current_subject_scope_is_rechecked_before_native_append() {
    let server = server().await;
    let js = server.jetstream().await;
    let config = config("SESSION_SCOPE_PROFILE", "fixture");
    let namespace = NatsToken::new("fixture").unwrap();
    let profile = SessionStreamProfile::new(config.name.clone(), namespace.clone(), 1).unwrap();
    let store = store(&js, config.clone(), profile).await;
    let first = SessionKey::new(namespace.clone(), "first").unwrap();
    store.append_stream(append(&first)).await.expect("valid profile append");

    js.update_stream(jetstream::stream::Config {
        subjects: vec!["fixture.session.sessions.>".into(), "foreign.>".into()],
        ..config
    })
    .await
    .expect("widen configured subjects");
    let next = SessionKey::new(namespace, "next").unwrap();
    let result = store.append_stream(append(&next)).await;
    assert!(
        matches!(
            result,
            Err(JetStreamStoreError::ResolveSubject(SessionNativeError::ProfileMismatch))
        ),
        "a widened scope must refuse a new native occurrence: {result:?}"
    );
    assert_eq!(store.events_stream().get_info().await.unwrap().state.messages, 1);
}

#[tokio::test]
async fn protected_name_namespace_and_replica_requirements_cannot_be_inferred_from_the_stream() {
    let server = server().await;
    let js = server.jetstream().await;
    let namespace = NatsToken::new("fixture").unwrap();
    let config = config("SESSION_PROTECTED_PROFILE", "fixture");
    let key = SessionKey::new(namespace.clone(), "parent").unwrap();
    let profile = SessionStreamProfile::new("ANOTHER_STREAM", namespace.clone(), 1).unwrap();
    let store = store(&js, config.clone(), profile).await;
    assert!(matches!(
        store
            .read_stream(ReadStreamRequest {
                stream_id: &key,
                from: ReadFrom::Beginning
            })
            .await,
        Err(JetStreamStoreError::ResolveSubject(SessionNativeError::ProfileMismatch))
    ));

    let stream = store.events_stream().clone();
    let snapshots = store.snapshot_bucket().clone();
    let profile = SessionStreamProfile::new(config.name, namespace.clone(), 2).unwrap();
    let store = JetStreamStore::builder(js.clone(), stream.clone(), snapshots.clone())
        .with_subject_resolver(SessionSubjectResolver::new(profile));
    assert!(matches!(
        store.append_stream(append(&key)).await,
        Err(JetStreamStoreError::ResolveSubject(SessionNativeError::ProfileMismatch))
    ));

    let profile = SessionStreamProfile::new("SESSION_PROTECTED_PROFILE", namespace, 1).unwrap();
    let store =
        JetStreamStore::builder(js, stream, snapshots).with_subject_resolver(SessionSubjectResolver::new(profile));
    let foreign = SessionKey::new(NatsToken::new("foreign").unwrap(), "parent").unwrap();
    assert!(matches!(
        store.append_stream(append(&foreign)).await,
        Err(JetStreamStoreError::ResolveSubject(
            SessionNativeError::NamespaceMismatch
        ))
    ));
    assert_eq!(store.events_stream().get_info().await.unwrap().state.messages, 0);
}

#[tokio::test]
async fn destructive_retention_and_missing_storage_bounds_refuse_native_writes() {
    let server = server().await;
    let js = server.jetstream().await;
    let mutations: [fn(&mut jetstream::stream::Config); 8] = [
        |config| config.discard = jetstream::stream::DiscardPolicy::Old,
        |config| config.max_messages_per_subject = 1,
        |config| config.deny_delete = false,
        |config| config.deny_purge = false,
        |config| config.allow_message_ttl = true,
        |config| config.max_bytes = -1,
        |config| config.max_messages = -1,
        |config| config.max_message_size = -1,
    ];
    for (index, mutate) in mutations.into_iter().enumerate() {
        let namespace = NatsToken::new(format!("fixture{index}")).unwrap();
        let name = format!("SESSION_UNSAFE_PROFILE_{index}");
        let mut config = config(&name, namespace.as_str());
        mutate(&mut config);
        let profile = SessionStreamProfile::new(name, namespace.clone(), 1).unwrap();
        let store = store(&js, config, profile).await;
        let key = SessionKey::new(namespace, "parent").unwrap();
        let result = store.append_stream(append(&key)).await;
        assert!(
            matches!(
                result,
                Err(JetStreamStoreError::ResolveSubject(SessionNativeError::ProfileMismatch))
            ),
            "unsafe profile case {index} must refuse a new occurrence: {result:?}"
        );
        assert_eq!(store.events_stream().get_info().await.unwrap().state.messages, 0);
    }
}

#[test]
fn invalid_protected_profile_inputs_are_refused_at_construction() {
    let namespace = NatsToken::new("fixture").unwrap();
    for name in ["", "bad.name", "bad name", "bad>name", "bad/name"] {
        assert!(matches!(
            SessionStreamProfile::new(name, namespace.clone(), 1),
            Err(SessionNativeError::InvalidProfile)
        ));
    }
    for replicas in [0, 6, usize::MAX] {
        assert!(matches!(
            SessionStreamProfile::new("SESSION", namespace.clone(), replicas),
            Err(SessionNativeError::InvalidProfile)
        ));
    }
}
