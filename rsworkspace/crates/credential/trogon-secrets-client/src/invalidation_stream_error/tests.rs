use std::time::Duration;

use async_nats::jetstream::stream::{Config, DiscardPolicy, RetentionPolicy, SubjectTransform};

use super::InvalidationStreamError;
use crate::INVALIDATION_LOOKBACK;
use crate::fixtures::invalidation_stream_config;

fn refused(change: fn(&mut Config)) -> InvalidationStreamError {
    let mut config = invalidation_stream_config();
    change(&mut config);
    InvalidationStreamError::check(&config).unwrap_err()
}

#[test]
fn refuses_a_max_age_below_the_lookback() {
    let refused = refused(|config| config.max_age = INVALIDATION_LOOKBACK - Duration::from_secs(1));

    assert_eq!(refused, InvalidationStreamError::MaxAge);
    assert_eq!(
        refused.to_string(),
        "the invalidation stream keeps notices for less than the lookback"
    );
}

#[test]
fn refuses_a_per_message_ttl() {
    let refused = refused(|config| config.allow_message_ttl = true);

    assert_eq!(refused, InvalidationStreamError::MessageTtl);
    assert_eq!(
        refused.to_string(),
        "the invalidation stream lets a notice expire on its own TTL"
    );
}

#[test]
fn refuses_a_per_subject_limit() {
    let refused = refused(|config| config.max_messages_per_subject = 1);

    assert_eq!(refused, InvalidationStreamError::MaxMessagesPerSubject);
    assert_eq!(
        refused.to_string(),
        "the invalidation stream keeps only the latest notices of each subject"
    );
}

#[test]
fn refuses_a_retention_other_than_limits() {
    for retention in [RetentionPolicy::Interest, RetentionPolicy::WorkQueue] {
        let mut config = invalidation_stream_config();
        config.retention = retention;

        let refused = InvalidationStreamError::check(&config).unwrap_err();

        assert_eq!(refused, InvalidationStreamError::Retention, "{retention:?}");
        assert_eq!(
            refused.to_string(),
            "the invalidation stream removes notices once they are consumed"
        );
    }
}

#[test]
fn refuses_a_stream_that_lets_a_notice_be_deleted() {
    let refused = refused(|config| config.deny_delete = false);

    assert_eq!(refused, InvalidationStreamError::DeletesAllowed);
    assert_eq!(refused.to_string(), "the invalidation stream lets a notice be deleted");
}

#[test]
fn refuses_a_stream_that_lets_its_notices_be_purged() {
    let refused = refused(|config| config.deny_purge = false);

    assert_eq!(refused, InvalidationStreamError::PurgesAllowed);
    assert_eq!(
        refused.to_string(),
        "the invalidation stream lets its notices be purged"
    );
}

#[test]
fn refuses_a_stream_that_allows_a_rollup() {
    let refused = refused(|config| config.allow_rollup = true);

    assert_eq!(refused, InvalidationStreamError::Rollup);
    assert_eq!(
        refused.to_string(),
        "the invalidation stream lets a rollup replace its notices"
    );
}

#[test]
fn refuses_a_stream_that_rewrites_its_subjects() {
    let refused = refused(|config| {
        config.subject_transform = Some(SubjectTransform {
            source: "secrets.invalidation.v1.*.>".to_owned(),
            destination: "fake.rewritten.>".to_owned(),
        });
    });

    assert_eq!(refused, InvalidationStreamError::SubjectTransform);
    assert_eq!(
        refused.to_string(),
        "the invalidation stream rewrites the subjects it stores"
    );
}

#[test]
fn leaves_capacity_and_longer_keeping_alone() {
    type Change = fn(&mut Config);
    let cases: [Change; 9] = [
        |config| config.discard = DiscardPolicy::New,
        |config| config.max_bytes = 1 << 20,
        |config| config.max_messages = 10_000,
        |config| config.max_messages_per_subject = -1,
        |config| config.max_age = Duration::ZERO,
        |config| config.max_age = INVALIDATION_LOOKBACK,
        |config| config.max_age = Duration::from_secs(86_400),
        |config| config.allow_direct = false,
        |config| config.subjects.push("fake.extra.subject".to_owned()),
    ];
    for change in cases {
        let mut config = invalidation_stream_config();
        change(&mut config);

        assert_eq!(InvalidationStreamError::check(&config), Ok(()), "{config:?}");
    }
}

#[test]
fn accepts_the_stream_the_secrets_service_creates() {
    assert_eq!(InvalidationStreamError::check(&invalidation_stream_config()), Ok(()));
}
