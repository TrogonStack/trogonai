use std::time::Duration;

use trogon_std::NonZeroDuration;

use super::ClaimRetention;
use crate::jetstream::stream_max_age::StreamMaxAge;

const GRACE: Duration = Duration::from_secs(24 * 60 * 60);

#[test]
fn tracking_a_bounded_stream_expires_after_retention_plus_grace() {
    let retention = ClaimRetention::tracking(StreamMaxAge::from_secs(604_800).unwrap(), GRACE);

    assert_eq!(
        retention,
        ClaimRetention::TracksStream {
            stream_max_age: NonZeroDuration::from_secs(604_800).unwrap(),
            grace: GRACE,
        }
    );
    assert_eq!(retention.bucket_max_age(), Duration::from_secs(604_800) + GRACE);
}

#[test]
fn tracking_an_unbounded_stream_never_expires() {
    let retention = ClaimRetention::tracking(StreamMaxAge::NoExpiry, GRACE);

    assert_eq!(retention, ClaimRetention::EventSourced);
    // Duration::ZERO is how NATS spells "no expiry".
    assert_eq!(retention.bucket_max_age(), Duration::ZERO);
}

#[test]
fn bucket_ttl_outlives_the_stream_so_a_live_message_can_resolve() {
    let stream = Duration::from_secs(604_800);
    let retention = ClaimRetention::tracking(StreamMaxAge::from_secs(stream.as_secs()).unwrap(), GRACE);

    assert!(retention.bucket_max_age() > stream);
}
