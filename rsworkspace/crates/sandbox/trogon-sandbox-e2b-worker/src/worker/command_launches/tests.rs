use trogon_sandbox_contract::LeaseError;

use super::{admit, covers, invalid_command_scope, require_exact_scope};

#[test]
fn a_request_its_signature_covers_is_admitted() {
    assert!(admit(true).is_ok());
}

#[test]
fn a_request_its_signature_does_not_cover_is_refused() {
    assert!(matches!(admit(false), Err(LeaseError::InvalidSpec { .. })));
}

#[test]
fn an_unsigned_cancellation_lacks_the_authority_it_needs() {
    assert!(matches!(
        require_exact_scope(None, "sandbox.provider.e2b.cancel_command_launch.lease-1", b"{}"),
        Err(LeaseError::InvalidSpec { .. })
    ));
}

#[test]
fn the_refusal_names_the_missing_authority() {
    assert!(matches!(
        invalid_command_scope(),
        LeaseError::InvalidSpec { reason } if reason.contains("verified assignment authority")
    ));
}

#[test]
fn an_unsigned_request_is_covered_by_the_authority_it_lacks() {
    assert!(covers(None, "sandbox.provider.e2b.run_command.lease-1", b"{}"));
}
