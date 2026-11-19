use super::RecordedMetrics;

#[test]
#[should_panic(expected = "readings is not a counter of whole numbers")]
fn refuses_to_read_an_instrument_that_is_not_a_whole_number_counter() {
    let recorded = RecordedMetrics::new();
    recorded.meter().f64_counter("readings").build().add(1.5, &[]);

    recorded.counts("readings");
}
