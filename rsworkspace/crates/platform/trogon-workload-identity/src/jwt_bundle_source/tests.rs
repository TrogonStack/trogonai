use std::time::Duration;

use super::*;
use crate::constants::JWT_BUNDLE_MAX_STALENESS;
use crate::tests::jwt_svid_tests::{bundle, ec_key};

fn set() -> JwtBundleSet {
    JwtBundleSet::new()
        .with_bundle(bundle("example.test", &[&ec_key("a")]))
        .unwrap()
}

#[test]
fn a_static_test_source_refuses_an_empty_set() {
    assert_eq!(
        JwtBundleSource::static_for_tests(JwtBundleSet::new()).unwrap_err(),
        crate::JwtVerifierError::NoBundles
    );
}

#[tokio::test(start_paused = true)]
async fn a_stopped_driver_lets_the_set_age_out() {
    let (source, driver) = JwtBundleSource::new(UnixEndpoint::new("unix:///nonexistent/workload.sock").unwrap());
    source.state.replace(set());
    drop(driver);
    tokio::time::advance(JWT_BUNDLE_MAX_STALENESS + Duration::from_millis(1)).await;
    assert_eq!(source.current().unwrap_err(), JwtBundleSourceError::Stale);
}
