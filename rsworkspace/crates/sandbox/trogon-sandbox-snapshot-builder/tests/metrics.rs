use prometheus::Registry;
use trogon_sandbox_snapshot_builder::SnapshotBuilderMetrics;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn exposition(registry: &Registry) -> Result<String, Box<dyn std::error::Error>> {
    let mut buffer = Vec::new();
    prometheus::Encoder::encode(&prometheus::TextEncoder::new(), &registry.gather(), &mut buffer)?;
    Ok(String::from_utf8(buffer)?)
}

#[test]
fn a_second_registration_on_the_same_registry_is_refused() -> TestResult {
    let registry = Registry::new();
    SnapshotBuilderMetrics::register(&registry)?;
    assert!(SnapshotBuilderMetrics::register(&registry).is_err());
    Ok(())
}

#[test]
fn gauges_are_exposed_from_the_start_and_counters_once_labelled() -> TestResult {
    let registry = Registry::new();
    let metrics = SnapshotBuilderMetrics::register(&registry)?;

    let text = exposition(&registry)?;
    assert!(text.contains("snapshot_builder_inflight 0"), "{text}");
    assert!(text.contains("snapshot_builder_queue_depth 0"), "{text}");
    assert!(!text.contains("snapshot_builder_pushed_total{"), "{text}");

    metrics.pushed_total.with_label_values(&["sandbox_rootfs_v1"]).inc();
    metrics.failed_total.with_label_values(&["tag_not_moved"]).inc_by(2);
    metrics.inflight.inc();
    metrics.queue_depth.set(5);

    let text = exposition(&registry)?;
    assert!(
        text.contains(r#"snapshot_builder_pushed_total{artifact_type="sandbox_rootfs_v1"} 1"#),
        "{text}"
    );
    assert!(
        text.contains(r#"snapshot_builder_failed_total{reason="tag_not_moved"} 2"#),
        "{text}"
    );
    assert!(text.contains("snapshot_builder_inflight 1"), "{text}");
    assert!(text.contains("snapshot_builder_queue_depth 5"), "{text}");
    Ok(())
}

#[test]
fn clones_share_the_same_series() -> TestResult {
    let metrics = SnapshotBuilderMetrics::register(&Registry::new())?;
    let clone = metrics.clone();

    clone.failed_total.with_label_values(&["permanent"]).inc();

    assert_eq!(metrics.failed_total.with_label_values(&["permanent"]).get(), 1);
    Ok(())
}
