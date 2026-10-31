use super::*;

fn tolerance() -> NonZeroDuration {
    NonZeroDuration::from_secs(300).unwrap()
}

#[test]
fn a_stream_without_expiry_gets_twice_the_tolerance() {
    assert_eq!(
        Duration::from(DedupWindow::spanning(tolerance(), StreamMaxAge::NoExpiry)),
        Duration::from_secs(600)
    );
}

#[test]
fn a_long_lived_stream_gets_twice_the_tolerance() {
    let max_age = StreamMaxAge::from_secs(86_400).unwrap();
    assert_eq!(
        Duration::from(DedupWindow::spanning(tolerance(), max_age)),
        Duration::from_secs(600)
    );
}

#[test]
fn a_stream_that_expires_sooner_caps_the_window_at_its_max_age() {
    let max_age = StreamMaxAge::from_secs(400).unwrap();
    assert_eq!(
        Duration::from(DedupWindow::spanning(tolerance(), max_age)),
        Duration::from_secs(400)
    );
}

#[test]
fn a_tolerance_too_large_to_double_saturates() {
    let tolerance = NonZeroDuration::from_secs(u64::MAX).unwrap();
    assert_eq!(
        Duration::from(DedupWindow::spanning(tolerance, StreamMaxAge::NoExpiry)),
        Duration::MAX
    );
}
