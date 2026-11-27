use async_nats::jetstream::kv;
use bytes::Bytes;

use super::{LeaseRenewInterval, LeaseTtl, NatsKvLease, NatsKvLeaseConfig, ReleaseLease, TryAcquireLease};
use crate::test_support::JetStreamTestServer;

const BUCKET: &str = "trogon-leases-test";
const KEY: &str = "leader";

fn config() -> NatsKvLeaseConfig {
    NatsKvLeaseConfig::new(
        BUCKET,
        KEY,
        LeaseTtl::from_secs(30).expect("non-zero"),
        LeaseRenewInterval::from_secs(10).expect("non-zero"),
    )
    .expect("renew interval below ttl")
}

#[tokio::test]
async fn a_provisioned_lease_is_exclusive_until_its_holder_releases_it() {
    let server = JetStreamTestServer::start().await;
    let lease = NatsKvLease::provision(&server.jetstream().await, &config())
        .await
        .unwrap();

    let revision = lease.try_acquire(Bytes::from_static(b"node-a")).await.unwrap();
    assert!(
        lease.try_acquire(Bytes::from_static(b"node-b")).await.is_err(),
        "a held lease must not be acquired again"
    );

    lease.release(revision).await.unwrap();

    lease
        .try_acquire(Bytes::from_static(b"node-b"))
        .await
        .expect("a released lease must be acquirable");
}

#[tokio::test]
async fn a_lease_over_an_existing_bucket_is_exclusive_until_released() {
    let server = JetStreamTestServer::start().await;
    let store = server
        .jetstream()
        .await
        .create_key_value(kv::Config {
            bucket: BUCKET.to_owned(),
            history: 1,
            ..Default::default()
        })
        .await
        .unwrap();
    let lease = NatsKvLease::try_new(store, KEY).unwrap();

    let revision = lease.try_acquire(Bytes::from_static(b"node-a")).await.unwrap();
    assert!(lease.try_acquire(Bytes::from_static(b"node-b")).await.is_err());

    lease.release(revision).await.unwrap();

    lease
        .try_acquire(Bytes::from_static(b"node-b"))
        .await
        .expect("a released lease must be acquirable");
}

#[tokio::test]
async fn releasing_with_a_stale_revision_leaves_the_lease_held() {
    let server = JetStreamTestServer::start().await;
    let lease = NatsKvLease::provision(&server.jetstream().await, &config())
        .await
        .unwrap();

    let revision = lease.try_acquire(Bytes::from_static(b"node-a")).await.unwrap();

    assert!(
        lease.release(revision + 1).await.is_err(),
        "a release must name the revision that holds the lease"
    );
    assert!(
        lease.try_acquire(Bytes::from_static(b"node-b")).await.is_err(),
        "a failed release must leave the lease held"
    );
}
