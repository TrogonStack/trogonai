use std::time::{Duration, UNIX_EPOCH};

use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, ReplayPolicy};
use time::OffsetDateTime;

use super::consumer_config;
use crate::INVALIDATION_LOOKBACK;
use crate::fixtures::now;

#[test]
fn starts_no_earlier_than_the_epoch() {
    let config = consumer_config(UNIX_EPOCH + Duration::from_secs(1));

    assert_eq!(
        config.deliver_policy,
        DeliverPolicy::ByStartTime {
            start_time: OffsetDateTime::UNIX_EPOCH
        }
    );
}

#[test]
fn reads_every_tenant_from_the_lookback_as_an_ordered_consumer_does() {
    let config = consumer_config(now());

    assert_eq!(config.filter_subject, "secrets.invalidation.v1.*.>");
    assert_eq!(
        config.deliver_policy,
        DeliverPolicy::ByStartTime {
            start_time: OffsetDateTime::from(now() - INVALIDATION_LOOKBACK)
        }
    );
    assert_eq!(config.replay_policy, ReplayPolicy::Instant);
    assert_eq!(config.ack_policy, AckPolicy::None);
    assert!(config.memory_storage);
    assert_eq!(config.num_replicas, 1);
    assert_eq!(config.durable_name, None);
    assert_eq!(config.name, None);
}
