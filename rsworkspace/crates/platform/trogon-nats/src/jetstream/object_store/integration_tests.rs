use std::time::Duration;

use trogon_std::NonZeroDuration;

use super::{NatsObjectStore, ProvisionObjectStoreError};
use crate::jetstream::claim_bucket::ClaimBucket;
use crate::jetstream::claim_check::ClaimResolver;
use crate::jetstream::claim_retention::ClaimRetention;
use crate::jetstream::stream_max_age::StreamMaxAge;
use crate::test_support::JetStreamTestServer;

const BUCKET: &str = "trogon-claims-test";
const GRACE_SECS: u64 = 3600;

fn claim_bucket() -> ClaimBucket {
    ClaimBucket::new(BUCKET).expect("valid bucket name")
}

fn tracking(stream_secs: u64) -> ClaimRetention {
    ClaimRetention::tracking(
        StreamMaxAge::from_secs(stream_secs).expect("non-zero"),
        NonZeroDuration::from_secs(GRACE_SECS).expect("non-zero"),
    )
}

async fn backing_stream_max_age(js: &async_nats::jetstream::Context) -> Duration {
    js.get_stream(format!("OBJ_{BUCKET}"))
        .await
        .expect("get backing stream")
        .cached_info()
        .config
        .max_age
}

#[tokio::test]
async fn provisioning_an_existing_bucket_reconciles_its_retention() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;

    let short = tracking(3600);
    NatsObjectStore::provision_claim_bucket(&js, claim_bucket(), short)
        .await
        .expect("first provision");
    assert_eq!(backing_stream_max_age(&js).await, short.bucket_max_age());

    // Re-provisioning the now-existing bucket with a longer retention must take
    // effect, not silently keep the old TTL.
    let long = tracking(3 * 3600);
    assert_ne!(long.bucket_max_age(), short.bucket_max_age());
    NatsObjectStore::provision_claim_bucket(&js, claim_bucket(), long)
        .await
        .expect("re-provision wider");
    assert_eq!(backing_stream_max_age(&js).await, long.bucket_max_age());

    // Re-provisioning with a shorter retention must NOT shrink the bucket:
    // older, still-deliverable messages could reference claims that would
    // otherwise expire early.
    NatsObjectStore::provision_claim_bucket(&js, claim_bucket(), short)
        .await
        .expect("re-provision narrower");
    assert_eq!(backing_stream_max_age(&js).await, long.bucket_max_age());
}

#[tokio::test]
async fn invalid_bucket_name_preserves_the_creation_error() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let result = NatsObjectStore::provision(
        &js,
        async_nats::jetstream::object_store::Config {
            bucket: "invalid.bucket".to_owned(),
            ..Default::default()
        },
    )
    .await;
    assert!(matches!(
        result,
        Err(ProvisionObjectStoreError::Create(source))
            if source.kind() == async_nats::jetstream::context::CreateKeyValueErrorKind::InvalidStoreName
    ));
}

/// What the binding is for: the bucket a resolver checks incoming claims
/// against is the bucket it actually opened, because one call produced both.
/// Labelling a handle with some other name is not merely wrong here, it is
/// unwritable, which is why there is no negative case to pair with this.
#[tokio::test]
async fn a_resolver_reads_the_bucket_its_binding_opened() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    NatsObjectStore::provision_claim_bucket(&js, claim_bucket(), tracking(3600))
        .await
        .expect("provision claim bucket");

    let binding = NatsObjectStore::bind_claim_bucket(&js, claim_bucket())
        .await
        .expect("bind claim bucket");
    assert_eq!(binding.bucket(), &claim_bucket());
    assert_eq!(ClaimResolver::new(binding).bucket(), &claim_bucket());
}
