use super::*;
use crate::telemetry::recorded_metrics::RecordedMetrics;
use trogon_semconv::metric::EGRESS_PROXY_CONNECTIONS_ACCEPTED;

#[test]
fn nothing_is_counted_before_a_connection_is_accepted() {
    let recorded = RecordedMetrics::new();
    ConnectionMetrics::new(&recorded.meter());
    assert!(recorded.counts(EGRESS_PROXY_CONNECTIONS_ACCEPTED).is_empty());
}

#[test]
fn each_accepted_connection_is_counted_under_its_credential_source() {
    let recorded = RecordedMetrics::new();
    let metrics = ConnectionMetrics::new(&recorded.meter());

    metrics.record_accepted(CredentialSource::WorkloadApi);
    metrics.record_accepted(CredentialSource::WorkloadApi);
    metrics.record_accepted(CredentialSource::StaticFile);

    assert_eq!(
        recorded.counts("egress_proxy.connections.accepted"),
        [
            (vec!["egress_proxy.credential_source=static_file".to_owned()], 1),
            (vec!["egress_proxy.credential_source=workload_api".to_owned()], 2),
        ]
    );
}

#[test]
fn debug_output_names_the_metrics_without_their_instruments() {
    let recorded = RecordedMetrics::new();
    assert_eq!(
        format!("{:?}", ConnectionMetrics::new(&recorded.meter())),
        "ConnectionMetrics { .. }"
    );
}
