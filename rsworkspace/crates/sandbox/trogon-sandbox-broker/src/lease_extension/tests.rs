use std::error::Error;
use std::time::{Duration, SystemTime};

use trogon_sandbox_contract::LeaseError;

use super::validate_lease_extension_with_max;

#[test]
fn extension_within_the_cap_moves_the_deadline() -> Result<(), Box<dyn Error>> {
    let created = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
    let deadline = created + Duration::from_secs(60);
    let extended = validate_lease_extension_with_max(created, deadline, 30, 120)?;
    assert_eq!(extended, deadline + Duration::from_secs(30));
    Ok(())
}

#[test]
fn extension_past_the_cap_is_refused() {
    let created = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
    let deadline = created + Duration::from_secs(100);
    let refused = validate_lease_extension_with_max(created, deadline, 30, 120);
    assert_eq!(refused, Err(LeaseError::WouldExceedMaxLifetime { max: 120 }));
}

#[test]
fn deadline_before_creation_counts_as_zero_lifetime() -> Result<(), Box<dyn Error>> {
    let created = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
    let deadline = SystemTime::UNIX_EPOCH;
    let extended = validate_lease_extension_with_max(created, deadline, 5, 1)?;
    assert_eq!(extended, deadline + Duration::from_secs(5));
    Ok(())
}
