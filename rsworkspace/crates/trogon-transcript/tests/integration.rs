use bytes::Bytes;
use testcontainers_modules::{
    nats::Nats,
    testcontainers::{runners::AsyncRunner, ImageExt},
};
use trogon_transcript::{
    publisher::NatsTranscriptPublisher,
    session::Session,
    store::TranscriptStore,
    entry::TranscriptEntry,
    error::TranscriptError,
};

async fn setup() -> (async_nats::Client, impl Drop) {
    let container = Nats::default()
        .with_cmd(["-js"])
        .start()
        .await
        .expect("failed to start NATS");
    let port = container.get_host_port_ipv4(4222).await.expect("failed to get port");
    let client = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    (client, container)
}

#[tokio::test]
async fn provision_creates_transcripts_stream() {
    let (nats, _container) = setup().await;
    let js = async_nats::jetstream::new(nats);
    let store = TranscriptStore::new(js.clone());

    store.provision().await.expect("provision failed");

    // Stream should exist after provisioning.
    js.get_stream("TRANSCRIPTS")
        .await
        .expect("TRANSCRIPTS stream not found after provision");
}

#[tokio::test]
async fn provision_is_idempotent() {
    let (nats, _container) = setup().await;
    let js = async_nats::jetstream::new(nats);
    let store = TranscriptStore::new(js.clone());

    store.provision().await.expect("first provision failed");
    store.provision().await.expect("second provision should not fail");
}

#[tokio::test]
async fn append_and_query_entries() {
    let (nats, _container) = setup().await;
    let js = async_nats::jetstream::new(nats);

    let store = TranscriptStore::new(js.clone());
    store.provision().await.unwrap();

    let publisher = NatsTranscriptPublisher::new(js);
    let session = Session::new(publisher, "pr", "owner/repo/1");

    session.append_user_message("Please review this PR.", None).await.unwrap();
    session.append_assistant_message("LGTM", Some(10)).await.unwrap();

    let entries = store.query("pr", "owner/repo/1").await.unwrap();
    assert_eq!(entries.len(), 2);

    assert!(matches!(&entries[0], TranscriptEntry::Message { role: trogon_transcript::Role::User, content, .. } if content == "Please review this PR."));
    assert!(matches!(&entries[1], TranscriptEntry::Message { role: trogon_transcript::Role::Assistant, tokens: Some(10), .. }));
}

#[tokio::test]
async fn query_spans_all_sessions_for_entity() {
    let (nats, _container) = setup().await;
    let js = async_nats::jetstream::new(nats);

    let store = TranscriptStore::new(js.clone());
    store.provision().await.unwrap();

    let publisher = NatsTranscriptPublisher::new(js);

    let session1 = Session::new(publisher.clone(), "pr", "owner/repo/1");
    session1.append_user_message("First event.", None).await.unwrap();

    let session2 = Session::new(publisher.clone(), "pr", "owner/repo/1");
    session2.append_user_message("Second event.", None).await.unwrap();

    // query returns entries from both sessions.
    let entries = store.query("pr", "owner/repo/1").await.unwrap();
    assert_eq!(entries.len(), 2);
}

#[tokio::test]
async fn replay_returns_only_the_requested_session() {
    let (nats, _container) = setup().await;
    let js = async_nats::jetstream::new(nats);

    let store = TranscriptStore::new(js.clone());
    store.provision().await.unwrap();

    let publisher = NatsTranscriptPublisher::new(js);

    let session1 = Session::new(publisher.clone(), "pr", "owner/repo/2");
    session1.append_user_message("session-one message", None).await.unwrap();
    let session1_id = session1.id().to_string();

    let session2 = Session::new(publisher.clone(), "pr", "owner/repo/2");
    session2.append_user_message("session-two message", None).await.unwrap();

    // Replay only session1.
    let entries = store.replay("pr", "owner/repo/2", &session1_id).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert!(matches!(&entries[0], TranscriptEntry::Message { content, .. } if content == "session-one message"));
}

#[tokio::test]
async fn query_different_entities_are_independent() {
    let (nats, _container) = setup().await;
    let js = async_nats::jetstream::new(nats);

    let store = TranscriptStore::new(js.clone());
    store.provision().await.unwrap();

    let publisher = NatsTranscriptPublisher::new(js);

    let s1 = Session::new(publisher.clone(), "pr", "owner/repo/10");
    s1.append_user_message("entity-10", None).await.unwrap();

    let s2 = Session::new(publisher.clone(), "pr", "owner/repo/20");
    s2.append_user_message("entity-20", None).await.unwrap();

    let e10 = store.query("pr", "owner/repo/10").await.unwrap();
    let e20 = store.query("pr", "owner/repo/20").await.unwrap();

    assert_eq!(e10.len(), 1);
    assert_eq!(e20.len(), 1);
    assert!(matches!(&e10[0], TranscriptEntry::Message { content, .. } if content == "entity-10"));
    assert!(matches!(&e20[0], TranscriptEntry::Message { content, .. } if content == "entity-20"));
}

#[tokio::test]
async fn all_entry_types_round_trip() {
    let (nats, _container) = setup().await;
    let js = async_nats::jetstream::new(nats);

    let store = TranscriptStore::new(js.clone());
    store.provision().await.unwrap();

    let publisher = NatsTranscriptPublisher::new(js);
    let session = Session::new(publisher, "router", "global");

    session.append_user_message("user msg", None).await.unwrap();
    session.append_assistant_message("assistant reply", Some(5)).await.unwrap();
    session.append_system_message("system context").await.unwrap();
    session
        .append_tool_call("search", serde_json::json!({"q": "foo"}), serde_json::json!({"results": []}), 42)
        .await
        .unwrap();
    session
        .append_routing_decision("gateway", "PrActor", "PR event")
        .await
        .unwrap();
    session
        .append_sub_agent_spawn("router/global", "SecurityActor", "security_analysis")
        .await
        .unwrap();

    let entries = store.query("router", "global").await.unwrap();
    assert_eq!(entries.len(), 6);

    assert!(matches!(&entries[2], TranscriptEntry::Message { role: trogon_transcript::Role::System, .. }));
    assert!(matches!(&entries[3], TranscriptEntry::ToolCall { name, duration_ms: 42, .. } if name == "search"));
    assert!(matches!(&entries[4], TranscriptEntry::RoutingDecision { to, .. } if to == "PrActor"));
    assert!(matches!(&entries[5], TranscriptEntry::SubAgentSpawn { capability, .. } if capability == "security_analysis"));
}

/// Publish a non-JSON payload directly into the TRANSCRIPTS stream (simulating
/// a corrupted or schema-incompatible entry) and verify that query() surfaces a
/// deserialization error rather than panicking or silently skipping the entry.
#[tokio::test]
async fn corrupted_entry_in_stream_returns_deserialization_error() {
    let (nats, _container) = setup().await;
    let js = async_nats::jetstream::new(nats);

    let store = TranscriptStore::new(js.clone());
    store.provision().await.unwrap();

    // Inject a corrupt entry directly into JetStream on the expected subject.
    js.publish(
        "transcripts.pr.corrupt-entity.session-bad",
        Bytes::from_static(b"this is not json at all {{{{"),
    )
    .await
    .unwrap()
    .await
    .unwrap();

    let result = store.query("pr", "corrupt-entity").await;

    assert!(
        matches!(result, Err(TranscriptError::Deserialization(_))),
        "expected Deserialization error, got: {result:?}"
    );
}

/// Publishing when the TRANSCRIPTS stream has not been provisioned causes the
/// JetStream ack to fail. The error must bubble up as `TranscriptError::Publish`
/// rather than panicking or being silently swallowed.
#[tokio::test]
async fn session_append_returns_publish_error_when_stream_not_provisioned() {
    let (nats, _container) = setup().await;
    let js = async_nats::jetstream::new(nats);
    // Deliberately skip TranscriptStore::provision() so no TRANSCRIPTS stream exists.
    let publisher = NatsTranscriptPublisher::new(js);
    let session = Session::new(publisher, "pr", "no-stream-entity");

    let err = session.append_user_message("hello", None).await.unwrap_err();

    assert!(
        matches!(err, TranscriptError::Publish(_)),
        "expected Publish error when no stream is provisioned, got: {err:?}"
    );
}

/// A stream that contains one valid and one corrupted entry should fail on the
/// corrupted entry — the store does not silently skip bad payloads.
#[tokio::test]
async fn corrupted_entry_among_valid_entries_returns_error() {
    let (nats, _container) = setup().await;
    let js = async_nats::jetstream::new(nats.clone());

    let store = TranscriptStore::new(js.clone());
    store.provision().await.unwrap();

    // Write one valid entry via the normal Session path.
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let session = Session::new(publisher, "pr", "mixed-entity");
    session.append_user_message("ok", None).await.unwrap();

    // Then inject a corrupt entry for the same entity (different session).
    js.publish(
        "transcripts.pr.mixed-entity.session-bad",
        Bytes::from_static(b"{{bad}}"),
    )
    .await
    .unwrap()
    .await
    .unwrap();

    // query fetches all entries for the entity — it should error on the bad one.
    let result = store.query("pr", "mixed-entity").await;
    assert!(
        matches!(result, Err(TranscriptError::Deserialization(_))),
        "expected Deserialization error on mixed stream, got: {result:?}"
    );
}
