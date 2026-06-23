use super::*;

#[test]
fn formats_whole_second_durations() {
    assert_eq!(format_go_duration(Duration::from_secs(30)).unwrap(), "30s");
    assert_eq!(format_go_duration(Duration::from_secs(90)).unwrap(), "1m30s");
    assert_eq!(format_go_duration(Duration::from_secs(3661)).unwrap(), "1h1m1s");
    assert_eq!(format_go_duration(Duration::from_secs(86_400)).unwrap(), "24h");
}

#[test]
fn formats_sub_second_components() {
    assert_eq!(format_go_duration(Duration::from_millis(1500)).unwrap(), "1s500ms");
    assert_eq!(format_go_duration(Duration::from_micros(500)).unwrap(), "500us");
    assert_eq!(format_go_duration(Duration::from_nanos(7)).unwrap(), "7ns");
    assert_eq!(format_go_duration(Duration::ZERO).unwrap(), "0s");
}

#[test]
fn rejects_durations_larger_than_the_go_maximum() {
    let too_large = Duration::from_secs(10_000_000_000);
    assert!(too_large.as_nanos() > GO_DURATION_MAX_NANOS);

    let error = format_go_duration(too_large).unwrap_err();
    assert!(matches!(error, GoDurationError::TooLarge { .. }));
    assert!(error.to_string().contains("exceeds the maximum Go duration"));
}
