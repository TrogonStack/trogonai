use std::error::Error;

use super::*;

#[test]
fn every_identifier_is_fresh() -> Result<(), Box<dyn Error>> {
    assert_ne!(fresh_lease_id(), fresh_lease_id());
    assert_ne!(fresh_attempt_id()?, fresh_attempt_id()?);
    assert_ne!(fresh_request_key()?, fresh_request_key()?);
    assert_ne!(fresh_execution_id(), fresh_execution_id());
    Ok(())
}

#[test]
fn the_tenant_is_the_conformance_tenant() -> Result<(), Box<dyn Error>> {
    assert_eq!(tenant()?.as_str(), TENANT);
    Ok(())
}
