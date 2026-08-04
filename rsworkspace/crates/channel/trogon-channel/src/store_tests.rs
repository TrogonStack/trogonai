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
        agent_id: AgentId::new("default"),
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

    record.current_session = Some(AgentSessionId::new("sess-1"));
    store.update_conversation(&id, &record).await.expect("update");

    let (found_id, found) = store
        .conversation_for(&endpoint)
        .await
        .expect("conversation lookup")
        .expect("conversation is bound");

    assert_eq!(found_id, id);
    assert_eq!(found.current_session, Some(AgentSessionId::new("sess-1")));
    assert_eq!(found.principal, principal);
}
