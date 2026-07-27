use std::time::Duration;

use trogon_std::NonZeroDuration;

use super::NatsObjectStore;
use crate::jetstream::claim_retention::ClaimRetention;
use crate::jetstream::stream_max_age::StreamMaxAge;
use crate::test_support::JetStreamTestServer;

const BUCKET: &str = "trogon-claims-test";
const GRACE_SECS: u64 = 3600;

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
    NatsObjectStore::provision_claim_bucket(&js, BUCKET, short)
        .await
        .expect("first provision");
    assert_eq!(backing_stream_max_age(&js).await, short.bucket_max_age());

    // Re-provisioning the now-existing bucket with a longer retention must take
    // effect, not silently keep the old TTL.
    let long = tracking(3 * 3600);
    assert_ne!(long.bucket_max_age(), short.bucket_max_age());
    NatsObjectStore::provision_claim_bucket(&js, BUCKET, long)
        .await
        .expect("re-provision wider");
    assert_eq!(backing_stream_max_age(&js).await, long.bucket_max_age());

    // Re-provisioning with a shorter retention must NOT shrink the bucket:
    // older, still-deliverable messages could reference claims that would
    // otherwise expire early.
    NatsObjectStore::provision_claim_bucket(&js, BUCKET, short)
        .await
        .expect("re-provision narrower");
    assert_eq!(backing_stream_max_age(&js).await, long.bucket_max_age());
}
