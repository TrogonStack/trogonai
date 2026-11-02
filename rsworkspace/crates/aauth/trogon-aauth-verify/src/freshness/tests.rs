use super::*;

fn freshness(validity: u32, skew: u32) -> SignatureFreshness {
    SignatureFreshness::new(
        SignatureValidity::new(validity).expect("validity in range"),
        ClockTolerance::new(skew).expect("skew in range"),
    )
}

#[test]
fn signature_validity_accepts_one_second_through_the_ceiling() {
    assert_eq!(SignatureValidity::new(1).expect("one second").secs(), 1);
    assert_eq!(
        SignatureValidity::new(MAX_SIGNATURE_VALIDITY_SECS)
            .expect("ceiling")
            .secs(),
        MAX_SIGNATURE_VALIDITY_SECS
    );
}

#[test]
fn signature_validity_rejects_zero_and_anything_past_the_ceiling() {
    assert!(SignatureValidity::new(0).is_err());
    assert!(SignatureValidity::new(MAX_SIGNATURE_VALIDITY_SECS + 1).is_err());
}

#[test]
fn signature_validity_defaults_to_one_minute() {
    assert_eq!(SignatureValidity::default().secs(), 60);
}

#[test]
fn clock_tolerance_accepts_zero_through_the_ceiling() {
    assert_eq!(ClockTolerance::new(0).expect("zero").secs(), 0);
    assert_eq!(
        ClockTolerance::new(MAX_CLOCK_SKEW_SECS).expect("ceiling").secs(),
        MAX_CLOCK_SKEW_SECS
    );
}

#[test]
fn clock_tolerance_rejects_anything_past_the_ceiling() {
    assert!(ClockTolerance::new(MAX_CLOCK_SKEW_SECS + 1).is_err());
}

#[test]
fn clock_tolerance_defaults_to_thirty_seconds() {
    assert_eq!(ClockTolerance::default().secs(), 30);
}

#[test]
fn check_accepts_a_window_up_to_the_skew_past_its_expiry() {
    let freshness = freshness(60, 30);
    let window = SignatureWindow::new(1000, 1060);
    assert_eq!(window.created(), 1000);
    assert_eq!(window.expires(), 1060);
    assert_eq!(freshness.check(window, 1000), Ok(()));
    assert_eq!(freshness.check(window, 1090), Ok(()));
    assert_eq!(freshness.check(window, 1091), Err(FreshnessError::Expired));
}

#[test]
fn check_accepts_a_window_created_up_to_the_skew_ahead() {
    let freshness = freshness(60, 30);
    let window = SignatureWindow::new(1030, 1090);
    assert_eq!(freshness.check(window, 1000), Ok(()));
    let ahead = SignatureWindow::new(1031, 1091);
    assert_eq!(freshness.check(ahead, 1000), Err(FreshnessError::NotYetValid));
}

#[test]
fn check_rejects_a_window_longer_than_the_validity() {
    let freshness = freshness(60, 30);
    assert_eq!(
        freshness.check(SignatureWindow::new(1000, 1061), 1000),
        Err(FreshnessError::InvalidWindow)
    );
    assert_eq!(freshness.check(SignatureWindow::new(1000, 1060), 1000), Ok(()));
}

#[test]
fn check_accepts_the_ceiling_window_when_configured_for_it() {
    let freshness = freshness(MAX_SIGNATURE_VALIDITY_SECS, 30);
    let window = SignatureWindow::new(1000, 1000 + i64::from(MAX_SIGNATURE_VALIDITY_SECS));
    assert_eq!(freshness.check(window, 1000), Ok(()));
}

#[test]
fn check_rejects_a_window_that_does_not_end_after_it_starts() {
    let freshness = freshness(60, 30);
    assert_eq!(
        freshness.check(SignatureWindow::new(1000, 1000), 1000),
        Err(FreshnessError::InvalidWindow)
    );
    assert_eq!(
        freshness.check(SignatureWindow::new(1000, 990), 1000),
        Err(FreshnessError::InvalidWindow)
    );
}

#[test]
fn check_rejects_a_window_whose_span_overflows() {
    let freshness = freshness(60, 30);
    assert_eq!(
        freshness.check(SignatureWindow::new(i64::MIN, i64::MAX), 0),
        Err(FreshnessError::InvalidWindow)
    );
}

#[test]
fn check_with_zero_skew_rejects_one_second_either_side() {
    let freshness = freshness(60, 0);
    let window = SignatureWindow::new(1000, 1060);
    assert_eq!(freshness.check(window, 999), Err(FreshnessError::NotYetValid));
    assert_eq!(freshness.check(window, 1061), Err(FreshnessError::Expired));
}

#[test]
fn record_ttl_covers_the_window_and_the_skew_on_both_sides() {
    assert_eq!(SignatureFreshness::default().record_ttl_secs(), 120);
    assert_eq!(
        freshness(MAX_SIGNATURE_VALIDITY_SECS, MAX_CLOCK_SKEW_SECS).record_ttl_secs(),
        crate::constants::REPLAY_RECORD_LIFETIME_SECS
    );
}
