use std::time::Duration;

use nkeys::XKey;

use super::ReleaseClientConfig;
use crate::fixtures::runtime_service;
use crate::{DEFAULT_RELEASE_TIMEOUT, ReleaseTimeout, ServiceXkey};

fn service_xkey() -> ServiceXkey {
    ServiceXkey::from_public_key(&XKey::new().public_key()).unwrap()
}

#[test]
fn waits_the_default_timeout_unless_told_otherwise() {
    let config = ReleaseClientConfig::new(runtime_service(), service_xkey());

    assert_eq!(config.timeout().get(), DEFAULT_RELEASE_TIMEOUT);
}

#[test]
fn keeps_what_it_was_configured_with() {
    let pinned = service_xkey();
    let timeout = ReleaseTimeout::new(Duration::from_millis(750)).unwrap();

    let config = ReleaseClientConfig::new(runtime_service(), pinned.clone()).with_timeout(timeout);

    assert_eq!(config.runtime_service(), &runtime_service());
    assert_eq!(config.service_xkey().public_key(), pinned.public_key());
    assert_eq!(config.timeout(), timeout);
}
