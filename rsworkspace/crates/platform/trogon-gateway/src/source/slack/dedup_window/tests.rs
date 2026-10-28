use super::*;

#[test]
fn the_window_covers_slacks_five_minute_retry() {
    assert!(DEDUP_WINDOW > Duration::from_secs(5 * 60));
}

#[test]
fn a_stream_without_expiry_gets_the_full_window() {
    assert_eq!(
        Duration::from(DedupWindow::within(StreamMaxAge::NoExpiry)),
        DEDUP_WINDOW
    );
}

#[test]
fn a_long_lived_stream_gets_the_full_window() {
    let max_age = StreamMaxAge::from_secs(86_400).unwrap();
    assert_eq!(Duration::from(DedupWindow::within(max_age)), DEDUP_WINDOW);
}

#[test]
fn a_stream_that_expires_sooner_caps_the_window_at_its_max_age() {
    let max_age = StreamMaxAge::from_secs(60).unwrap();
    assert_eq!(Duration::from(DedupWindow::within(max_age)), Duration::from_secs(60));
}
