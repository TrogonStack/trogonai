use std::time::Duration;

use super::*;

fn access(expires_at: Option<u64>) -> GatewayAccess {
    GatewayAccess {
        host: "gateway.test".to_owned(),
        port: 2222,
        token: "tok123".to_owned(),
        expires_at,
    }
}

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

#[test]
fn a_dated_token_is_good_until_the_refresh_margin_before_its_expiry() {
    let now = Instant::now();
    let dated = access(Some(1_000));
    assert!(dated.usable(now, now, at(900)));
    assert!(!dated.usable(now, now, at(940)));
    assert!(!dated.usable(now, now, at(2_000)));
}

#[test]
fn an_undated_token_is_good_for_fifty_minutes() {
    let minted = Instant::now();
    let undated = access(None);
    assert!(undated.usable(minted, minted + Duration::from_secs(49 * 60), at(0)));
    assert!(!undated.usable(minted, minted + Duration::from_secs(50 * 60), at(0)));
}

#[test]
fn debug_hides_the_token() {
    let shown = format!("{:?}", access(None));
    assert!(!shown.contains("tok123"));
    assert!(shown.contains("gateway.test"));
}
