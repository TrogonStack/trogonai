use super::*;
use crate::agent_port::AgentSessionId;
use crate::conversation::AgentId;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_std::UuidV7Generator;

fn endpoint(peer: &str) -> Endpoint {
    Endpoint::new("telegram", "bot", peer).expect("endpoint")
}

fn principal(id: &str) -> PrincipalId {
    PrincipalId::new(id).expect("principal")
}

fn record(principal: &PrincipalId) -> ConversationRecord {
    ConversationRecord {
        principal: principal.clone(),
        agent_id: AgentId::new("default").expect("agent id"),
        current_session: None,
        created_at: 1,
        last_activity_at: 1,
    }
}

/// A bridge restarts far more often than it first starts, so opening the
/// existing buckets is the common path, not the exceptional one.
#[tokio::test]
async fn ensure_creates_its_buckets_then_reopens_them() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;

    let store = ChannelStore::ensure(&js, "first").await.expect("first ensure");
    let endpoint = endpoint("111");
    let principal = principal("user-1");
    store
        .link_endpoint(
            &principal,
            &PrincipalRecord {
                display_name: Some("Ada".to_string()),
            },
            &endpoint,
        )
        .await
        .expect("link endpoint");

    let reopened = ChannelStore::ensure(&js, "first").await.expect("second ensure");

    assert_eq!(
        reopened.principal_for(&endpoint).await.expect("principal lookup"),
        Some(principal),
        "the second ensure must reopen the buckets rather than replace them"
    );
}

/// Two replicas booting together both race the same four buckets, and neither
/// losing the race is a provisioning failure.
#[tokio::test]
async fn ensure_is_idempotent_under_concurrent_creation() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;

    let (first, second) = tokio::join!(ChannelStore::ensure(&js, "race"), ChannelStore::ensure(&js, "race"));

    first.expect("one concurrent ensure");
    second.expect("the other concurrent ensure");
}

/// The regression this guards: a get that failed without JetStream answering
/// says nothing about whether the bucket exists. Reading it as absence sends the
/// bridge on to `STREAM.CREATE`, which applies some divergent fields as an
/// in-place update, so a momentary read failure could reconfigure live storage.
/// An unreachable API prefix is the cheapest way to fail a get for a reason
/// other than absence.
#[tokio::test]
async fn an_unreadable_bucket_is_not_treated_as_a_missing_one() {
    let server = JetStreamTestServer::start().await;
    let unreachable = jetstream::with_prefix(server.client().await, "NOT.THE.API");

    // Matched rather than `expect_err`ed because a `ChannelStore` is not
    // `Debug`: it is four live KV handles.
    let Err(error) = ChannelStore::ensure(&unreachable, "unreachable").await else {
        panic!("ensure must fail when the buckets cannot be read");
    };

    assert!(
        matches!(error, ChannelStoreError::OpenBucket { .. }),
        "expected the read failure to surface, got {error:?}"
    );

    let js = server.jetstream().await;
    assert!(
        js.get_key_value("channel_principals_unreachable").await.is_err(),
        "no bucket should have been created off the back of a read failure"
    );
}

/// The whole point of the four buckets: a conversation survives a restart, and
/// its session pointer is replaceable in place.
#[tokio::test]
async fn a_conversation_round_trips_through_its_buckets() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let store = ChannelStore::ensure(&js, "roundtrip").await.expect("ensure");

    let endpoint = endpoint("222");
    let principal = principal("user-2");
    let mut record = record(&principal);
    let id = store
        .create_conversation(&endpoint, &record, &UuidV7Generator)
        .await
        .expect("create conversation");

    record.current_session = Some(AgentSessionId::new("sess-1").expect("session id"));
    store.update_conversation(&id, &record).await.expect("update");

    let (found_id, found) = store
        .conversation_for(&endpoint)
        .await
        .expect("conversation lookup")
        .expect("conversation is bound");

    assert_eq!(found_id, id);
    assert_eq!(
        found.current_session,
        Some(AgentSessionId::new("sess-1").expect("session id"))
    );
    assert_eq!(found.principal, principal);
}

/// A binding can outlive the conversation it points at (the record aged out,
/// or was deleted, while the binding survived), and that dangling state must
/// read as unbound rather than panic on a missing record.
#[tokio::test]
async fn a_binding_with_no_conversation_record_reads_as_unbound() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let store = ChannelStore::ensure(&js, "dangling").await.expect("ensure");

    let endpoint = endpoint("333");
    let id = ConversationId::from_string("gone").expect("conversation id");

    // No public API writes a binding without its conversation record, so the
    // dangling state is written straight to the private `bindings` bucket.
    store
        .bindings
        .put(endpoint.kv_key(), serde_json::to_vec(&id).expect("encode id").into())
        .await
        .expect("write dangling binding");

    assert!(
        store
            .conversation_for(&endpoint)
            .await
            .expect("conversation lookup")
            .is_none(),
        "a dangling binding must not be reported as a bound conversation"
    );
}

/// The exposure the write order creates: the record goes in first so no binding
/// is ever briefly visible pointing at nothing, which leaves a failed binding
/// able to strand a record instead. Each attempt mints a fresh id, so without
/// the rollback every redelivery of one message would leave one more
/// unreachable record behind.
#[tokio::test]
async fn a_failed_binding_takes_the_conversation_record_with_it() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let store = ChannelStore::ensure(&js, "rollback").await.expect("ensure");

    // Dropping the bucket fails the binding write and nothing else, which is
    // the only case the rollback is there for.
    js.delete_key_value("channel_bindings_rollback")
        .await
        .expect("drop the bindings bucket");

    let endpoint = endpoint("444");
    let Err(error) = store
        .create_conversation(&endpoint, &record(&principal("user-4")), &UuidV7Generator)
        .await
    else {
        panic!("create must fail when the binding cannot be written");
    };

    let conversation = match error {
        ChannelStoreError::BindEndpoint { conversation, .. } => conversation,
        other => panic!("expected the binding failure to surface, got {other:?}"),
    };

    assert!(
        store
            .conversations
            .get(conversation.as_str())
            .await
            .expect("read the rolled back record")
            .is_none(),
        "a conversation nothing can reach must not survive the call that failed to bind it"
    );
}

/// The rollback that keeps a failed bind from stranding a record can itself
/// fail, and then the record really is unreachable, so the error has to carry
/// both causes and the key an operator needs to sweep. A conversations bucket
/// that accepts one write per subject and refuses the next forces that
/// deterministically: the record write is the one it accepts, and the rollback's
/// delete marker is a second write to the same subject.
#[tokio::test]
async fn a_rollback_that_also_fails_reports_the_record_it_could_not_remove() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;

    // Hand-built rather than left to `ensure_bucket`, whose own config would
    // accept the rollback. `get_key_value` asks only for a per-subject limit of
    // at least one, so this still opens as the store's conversations bucket.
    js.create_stream(jetstream::stream::Config {
        name: "KV_channel_conversations_orphan".to_string(),
        subjects: vec!["$KV.channel_conversations_orphan.>".to_string()],
        max_messages_per_subject: 1,
        discard: jetstream::stream::DiscardPolicy::New,
        discard_new_per_subject: true,
        ..Default::default()
    })
    .await
    .expect("a conversations bucket that refuses a second write to one subject");

    let store = ChannelStore::ensure(&js, "orphan").await.expect("ensure");

    js.delete_key_value("channel_bindings_orphan")
        .await
        .expect("drop the bindings bucket so only the bind fails");

    let Err(error) = store
        .create_conversation(&endpoint("555"), &record(&principal("user-5")), &UuidV7Generator)
        .await
    else {
        panic!("create must fail when the binding cannot be written");
    };

    let ChannelStoreError::OrphanedConversation { conversation, .. } = error else {
        panic!("expected the failed rollback to surface as an orphaned record, got {error:?}");
    };

    assert!(
        store
            .conversations
            .get(conversation.as_str())
            .await
            .expect("read the record the rollback could not remove")
            .is_some(),
        "the error must name a record that really is still there to be swept"
    );
}

/// `ensure_is_idempotent_under_concurrent_creation` races two *identical*
/// configs, and neither side ever takes this arm: `STREAM.CREATE` only
/// errors when the stream that beat it has a different config, and an
/// identical race succeeds silently on both sides. Racing a bare create
/// against `ensure_bucket`'s own get-then-create for the same bucket name
/// reliably loses that race instead: the bare create skips the get's extra
/// round trip, so its differently-configured bucket already exists by the
/// time this store's own create is rejected.
#[tokio::test]
async fn a_bucket_created_with_a_different_config_between_the_get_and_the_create_is_still_opened() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let bucket = "conflict".to_string();

    // `ensure_bucket` is private and reachable only through `ChannelStore::ensure`,
    // so it is called directly here to race a single bucket instead of all four.
    let racing_create = js.create_key_value(jetstream::kv::Config {
        bucket: bucket.clone(),
        history: 1,
        storage: jetstream::stream::StorageType::File,
        ..Default::default()
    });

    let (ours, theirs) = tokio::join!(ensure_bucket(&js, bucket.clone()), racing_create);

    ours.expect("ensure_bucket must recover the bucket the race left behind");
    theirs.expect("the racing create must succeed for there to be anything to recover");

    let mut stream = js
        .get_stream(format!("KV_{bucket}"))
        .await
        .expect("the bucket the race created must still be there");
    let info = stream.info().await.expect("stream info");
    assert_eq!(
        info.config.max_messages_per_subject, 1,
        "the surviving config must be the racing create's, not ensure_bucket's own attempt"
    );
}

/// `STREAM.CREATE` also rejects a bucket for reasons that have nothing to do
/// with a name already in use, and that class of failure must surface as-is
/// rather than being mistaken for the recoverable race above. A stream that
/// already claims this bucket's subject space under a different name forces
/// exactly that: JetStream reports a subject overlap (error code 10065), not
/// the name-in-use conflict (10058) `is_create_key_value_already_exists`
/// looks for, and the claim sits there deterministically before
/// `ensure_bucket` ever runs, so there is no race to lose.
#[tokio::test]
async fn a_bucket_whose_subject_space_is_already_claimed_fails_to_create() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let bucket = "claimed".to_string();

    js.create_stream(jetstream::stream::Config {
        name: "squatter".to_string(),
        subjects: vec![format!("$KV.{bucket}.>")],
        ..Default::default()
    })
    .await
    .expect("claim the bucket's subject space under an unrelated stream name");

    let Err(error) = ensure_bucket(&js, bucket).await else {
        panic!("ensure_bucket must fail when STREAM.CREATE is rejected for a reason other than a name conflict");
    };

    assert!(
        matches!(error, ChannelStoreError::CreateBucket { .. }),
        "expected the create failure to surface as-is, got {error:?}"
    );
}

/// Line 105's arm: the already-exists recovery read can itself fail. Racing
/// a stream into existence with `max_messages_per_subject` below the minimum
/// a real KV config ever produces (`kv_to_stream_config` floors it at 1)
/// forces exactly that: the name conflict sends `ensure_bucket` to recover by
/// reading the bucket back, and that read rejects what it finds as not a
/// valid KV store rather than returning it.
#[tokio::test]
async fn a_recovery_read_that_also_fails_surfaces_as_a_bucket_read_failure() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let bucket = "conflict-broken".to_string();

    // Plain `create_stream`, not `create_key_value`, because the KV wrapper
    // floors `max_messages_per_subject` at 1 and could never produce this.
    let racing_create = js.create_stream(jetstream::stream::Config {
        name: format!("KV_{bucket}"),
        subjects: vec![format!("$KV.{bucket}.>")],
        max_messages_per_subject: 0,
        ..Default::default()
    });

    let (ours, theirs) = tokio::join!(ensure_bucket(&js, bucket.clone()), racing_create);

    theirs.expect("the racing create must win for there to be a conflicting bucket to recover");

    let Err(error) = ours else {
        panic!("ensure_bucket must fail when its own recovery read also fails");
    };
    assert!(
        matches!(error, ChannelStoreError::OpenBucket { .. }),
        "expected the recovery read failure to surface, got {error:?}"
    );
}
