use super::*;

#[test]
fn a_counter_is_read_back_with_its_attributes() {
    let recorded = RecordedMetrics::new();
    recorded
        .meter()
        .u64_counter("test.events")
        .build()
        .add(2, &[opentelemetry::KeyValue::new("kind", "a")]);

    assert_eq!(recorded.counts("test.events"), [(vec!["kind=a".to_owned()], 2)]);
}

#[test]
#[should_panic(expected = "test.ratio is not a counter of whole numbers")]
fn reading_a_counter_of_fractions_is_refused() {
    let recorded = RecordedMetrics::new();
    recorded.meter().f64_counter("test.ratio").build().add(0.5, &[]);

    recorded.counts("test.ratio");
}
