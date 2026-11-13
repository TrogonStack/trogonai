use std::time::{Duration, UNIX_EPOCH};

use super::*;

#[test]
fn reports_no_bundle_generation_and_its_expiry() {
    let valid_until = UNIX_EPOCH + Duration::from_secs(1_000);
    let release = SealedRelease::new(SealedValue::new(vec![9]).unwrap(), valid_until);

    assert_eq!(release.sealed_value().as_bytes(), &[9]);
    assert_eq!(release.bundle_generation(), 0);
    assert_eq!(release.valid_until(), valid_until);
}
