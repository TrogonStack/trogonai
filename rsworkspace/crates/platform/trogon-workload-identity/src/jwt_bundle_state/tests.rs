use super::*;
use crate::constants::JWT_BUNDLE_MAX_STALENESS;
use crate::tests::jwt_svid_tests::{bundle, ec_key};
use std::time::Duration;

fn set() -> JwtBundleSet {
    JwtBundleSet::new()
        .with_bundle(bundle("example.test", &[&ec_key("a")]))
        .unwrap()
}

fn idle_source() -> JwtBundleState {
    JwtBundleState::default()
}

#[tokio::test(start_paused = true)]
async fn offers_nothing_before_the_first_set_arrives() {
    let source = idle_source();
    assert_eq!(source.current().unwrap_err(), JwtBundleSourceError::NeverDelivered);
    source.lost();
    tokio::time::advance(JWT_BUNDLE_MAX_STALENESS * 2).await;
    assert_eq!(
        source.current().unwrap_err(),
        JwtBundleSourceError::NeverDelivered,
        "a loss before any delivery stays distinct from staleness"
    );
}

#[tokio::test(start_paused = true)]
async fn a_live_stream_never_goes_stale() {
    let source = idle_source();
    source.replace(set());
    tokio::time::advance(JWT_BUNDLE_MAX_STALENESS * 100).await;
    assert!(source.current().is_ok());
}

#[tokio::test(start_paused = true)]
async fn keeps_the_last_set_for_the_staleness_bound_after_the_stream_is_lost_then_fails_closed() {
    let source = idle_source();
    source.replace(set());
    source.lost();
    tokio::time::advance(JWT_BUNDLE_MAX_STALENESS).await;
    assert!(source.current().is_ok(), "the bound itself is still within tolerance");
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(source.current().unwrap_err(), JwtBundleSourceError::Stale);
}

#[tokio::test(start_paused = true)]
async fn failed_reconnect_attempts_do_not_restart_the_staleness_clock() {
    let source = idle_source();
    source.replace(set());
    source.lost();
    for _ in 0..4 {
        tokio::time::advance(JWT_BUNDLE_MAX_STALENESS / 3).await;
        source.lost();
    }
    assert_eq!(source.current().unwrap_err(), JwtBundleSourceError::Stale);
}

#[tokio::test(start_paused = true)]
async fn a_fresh_delivery_after_a_loss_makes_the_source_current_again() {
    let source = idle_source();
    source.replace(set());
    source.lost();
    tokio::time::advance(JWT_BUNDLE_MAX_STALENESS * 2).await;
    assert!(source.current().is_err());
    source.replace(set());
    assert!(source.current().is_ok());
    tokio::time::advance(JWT_BUNDLE_MAX_STALENESS * 2).await;
    assert!(source.current().is_ok());
}
