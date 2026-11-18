#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::Duration;

use async_nats::jetstream;
use trogon_decider_nats::{JetStreamStore, JetStreamStoreError};
use trogon_decider_runtime::{ReadFrom, ReadStreamRequest, StreamRead};
use trogon_nats::{NatsToken, test_support::JetStreamTestServer};
use trogon_session_store::{SessionKey, SessionNativeError, SessionStreamProfile, SessionSubjectResolver};

async fn server() -> JetStreamTestServer {
    match std::env::var("TROGON_SESSION_TEST_NETWORK") {
        Ok(network) => JetStreamTestServer::start_on_network(&network).await,
        Err(_) => JetStreamTestServer::start().await,
    }
}

#[tokio::test]
async fn current_retention_profile_is_rechecked_before_each_native_read() {
    let server = server().await;
    let js = server.jetstream().await;
    let config = jetstream::stream::Config {
        name: "SESSION_NATIVE_PROFILE".into(),
        subjects: vec!["fixture.session.sessions.>".into()],
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
    };
    let stream = js.create_stream(config.clone()).await.expect("create Session stream");
    let snapshots = js
        .create_key_value(jetstream::kv::Config {
            bucket: "SESSION_NATIVE_PROFILE_SNAPSHOTS".into(),
            ..Default::default()
        })
        .await
        .expect("create snapshots");
    let namespace = NatsToken::new("fixture").unwrap();
    let profile = SessionStreamProfile::new(config.name.clone(), namespace.clone(), 1).unwrap();
    let store = JetStreamStore::builder(js.clone(), stream, snapshots)
        .with_subject_resolver(SessionSubjectResolver::new(profile));
    let key = SessionKey::new(namespace, "parent").unwrap();
    assert!(
        store
            .read_stream(ReadStreamRequest { stream_id: &key, from: ReadFrom::Beginning })
            .await
            .expect("valid profile read")
            .events
            .is_empty()
    );

    js.update_stream(jetstream::stream::Config { max_age: Duration::from_secs(3600), ..config })
        .await
        .expect("withdraw retention guarantee");
    let result = store
        .read_stream(ReadStreamRequest { stream_id: &key, from: ReadFrom::Beginning })
        .await;
    assert!(
        matches!(result, Err(JetStreamStoreError::ResolveSubject(SessionNativeError::ProfileMismatch))),
        "a current retention violation must refuse the native read before replay: {result:?}"
    );
}
