use trogon_semconv::metric::{
    SECRETS_CLIENT_INVALIDATION_POSITION_LOSSES, SECRETS_CLIENT_RELEASE_CACHE_CAPACITY_SKIPS,
};

use super::ReleaseCacheMetrics;
use crate::InvalidationLoss;
use crate::telemetry::RecordedMetrics;

#[test]
fn records_nothing_until_the_reader_loses_its_place_or_the_cache_fills() {
    let recorded = RecordedMetrics::new();

    ReleaseCacheMetrics::new(&recorded.meter());

    assert!(recorded.counts(SECRETS_CLIENT_INVALIDATION_POSITION_LOSSES).is_empty());
    assert!(recorded.counts(SECRETS_CLIENT_RELEASE_CACHE_CAPACITY_SKIPS).is_empty());
}

#[test]
fn counts_each_loss_of_place_under_its_reason() {
    let recorded = RecordedMetrics::new();
    let metrics = ReleaseCacheMetrics::new(&recorded.meter());

    metrics.record_position_loss(&InvalidationLoss::Ended);
    metrics.record_position_loss(&InvalidationLoss::Ended);
    metrics.record_position_loss(&InvalidationLoss::Gap {
        expected: 2,
        arrived: 3,
    });

    assert_eq!(
        recorded.counts(SECRETS_CLIENT_INVALIDATION_POSITION_LOSSES),
        [
            (vec!["secrets.client.invalidation.loss_reason=ended".to_owned()], 2),
            (vec!["secrets.client.invalidation.loss_reason=gap".to_owned()], 1),
        ]
    );
}

#[test]
fn counts_each_release_served_without_being_held() {
    let recorded = RecordedMetrics::new();
    let metrics = ReleaseCacheMetrics::new(&recorded.meter());

    metrics.record_capacity_skip();
    metrics.record_capacity_skip();

    assert_eq!(
        recorded.counts(SECRETS_CLIENT_RELEASE_CACHE_CAPACITY_SKIPS),
        [(Vec::<String>::new(), 2)]
    );
}

#[test]
fn debug_output_names_the_metrics_without_their_instruments() {
    let recorded = RecordedMetrics::new();

    let metrics = ReleaseCacheMetrics::new(&recorded.meter());

    assert_eq!(format!("{metrics:?}"), "ReleaseCacheMetrics { .. }");
}
