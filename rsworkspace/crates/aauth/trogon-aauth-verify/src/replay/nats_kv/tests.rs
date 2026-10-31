use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream::kv;
use async_nats::jetstream::stream::StorageType;
use trogon_nats::test_support::JetStreamTestServer;

use super::*;

async fn provision(jetstream: &jetstream::Context, history: i64, max_age: Duration) {
    jetstream
        .create_key_value(kv::Config {
            bucket: AAUTH_REPLAY_BUCKET.to_string(),
            history,
            max_age,
            storage: StorageType::File,
            ..Default::default()
        })
        .await
        .expect("provision replay bucket");
}

async fn provisioned_store() -> (JetStreamTestServer, jetstream::Context, NatsKvReplayStore) {
    let server = JetStreamTestServer::start().await;
    let jetstream = server.jetstream().await;
    let required = ReplayBucketSpec::REQUIRED;
    provision(&jetstream, required.history(), required.max_age()).await;
    let store = NatsKvReplayStore::open(&jetstream)
        .await
        .expect("open provisioned replay bucket");
    (server, jetstream, store)
}

#[test]
fn required_spec_keeps_one_revision_for_three_minutes() {
    assert_eq!(ReplayBucketSpec::REQUIRED.history(), 1);
    assert_eq!(ReplayBucketSpec::REQUIRED.max_age(), Duration::from_secs(180));
}

#[tokio::test(flavor = "multi_thread")]
async fn open_refuses_a_missing_bucket() {
    let server = JetStreamTestServer::start().await;
    let jetstream = server.jetstream().await;

    let err = NatsKvReplayStore::open(&jetstream)
        .await
        .err()
        .expect("missing bucket refused");

    assert!(matches!(err, ReplayBucketError::Open(_)), "got {err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn open_refuses_a_bucket_keeping_more_history() {
    let server = JetStreamTestServer::start().await;
    let jetstream = server.jetstream().await;
    provision(&jetstream, 5, ReplayBucketSpec::REQUIRED.max_age()).await;

    let err = NatsKvReplayStore::open(&jetstream)
        .await
        .err()
        .expect("bucket with history 5 refused");

    let ReplayBucketError::Incompatible(incompatible) = err else {
        panic!("expected Incompatible, got {err:?}");
    };
    assert_eq!(incompatible.actual().history(), 5);
    assert_eq!(incompatible.expected(), ReplayBucketSpec::REQUIRED);
}

#[tokio::test(flavor = "multi_thread")]
async fn open_refuses_a_bucket_with_another_max_age() {
    let server = JetStreamTestServer::start().await;
    let jetstream = server.jetstream().await;
    provision(&jetstream, 1, Duration::from_secs(60)).await;

    let err = NatsKvReplayStore::open(&jetstream)
        .await
        .err()
        .expect("bucket with a 60s max age refused");

    let ReplayBucketError::Incompatible(incompatible) = err else {
        panic!("expected Incompatible, got {err:?}");
    };
    assert_eq!(incompatible.actual().max_age(), Duration::from_secs(60));
}

#[tokio::test(flavor = "multi_thread")]
async fn first_record_is_admitted_and_its_duplicate_refused() {
    let (_server, _jetstream, store) = provisioned_store().await;

    assert!(store.check_and_insert("jti.key-1.aa", 150).await.unwrap());
    assert!(!store.check_and_insert("jti.key-1.aa", 150).await.unwrap());
    assert!(store.check_and_insert("jti.key-2.aa", 150).await.unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_duplicates_admit_exactly_one() {
    let (_server, _jetstream, store) = provisioned_store().await;
    let store = Arc::new(store);

    let attempts: Vec<_> = (0..16)
        .map(|_| {
            let store = Arc::clone(&store);
            tokio::spawn(async move { store.check_and_insert("jti.key-1.contended", 150).await })
        })
        .collect();
    let mut admitted = 0;
    for attempt in attempts {
        if attempt.await.expect("attempt task").expect("store answered") {
            admitted += 1;
        }
    }

    assert_eq!(admitted, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn record_outliving_the_bucket_fails_closed() {
    let (_server, _jetstream, store) = provisioned_store().await;

    let err = store.check_and_insert("jti.key-1.long", 181).await.unwrap_err();

    assert!(matches!(err, ReplayError::Backend(_)), "got {err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn unreachable_bucket_fails_closed() {
    let (_server, jetstream, store) = provisioned_store().await;
    jetstream
        .delete_key_value(AAUTH_REPLAY_BUCKET)
        .await
        .expect("remove replay bucket");

    let err = store.check_and_insert("jti.key-1.gone", 150).await.unwrap_err();

    assert!(matches!(err, ReplayError::Backend(_)), "got {err:?}");
}
