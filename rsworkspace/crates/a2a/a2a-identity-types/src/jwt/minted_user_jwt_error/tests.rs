use std::error::Error as _;

use trogon_identity_types::{PrincipalIdError, PrincipalKindError, SubjectPrincipalError, SubjectPrincipalIdError};

use super::MintedUserJwtError;
use crate::error::JwtError;
use crate::jwt::MintedClaim;

#[test]
fn a_missing_claim_names_the_claim() {
    assert_eq!(
        MintedUserJwtError::MissingClaim(MintedClaim::PrincipalKind).to_string(),
        "minted user JWT has no principal_kind claim"
    );
    assert_eq!(
        MintedUserJwtError::MissingClaim(MintedClaim::CallerId).to_string(),
        "minted user JWT has no caller_id claim"
    );
}

#[test]
fn every_variant_that_wraps_a_cause_exposes_it_as_its_source() {
    for error in [
        MintedUserJwtError::Unreadable(JwtError::Decode("bad base64".into())),
        MintedUserJwtError::InvalidPrincipalId(PrincipalIdError::ContainsControlCharacter),
        MintedUserJwtError::InvalidPrincipalKind(PrincipalKindError::Unknown),
        MintedUserJwtError::InvalidSubjectPrincipal(SubjectPrincipalError::Id(SubjectPrincipalIdError::AlreadyHashed)),
    ] {
        assert!(!error.to_string().is_empty());
        assert!(error.source().is_some(), "{error:?} must expose its cause");
    }
}

#[test]
fn a_variant_without_a_cause_exposes_no_source() {
    for error in [
        MintedUserJwtError::MissingClaim(MintedClaim::CallerId),
        MintedUserJwtError::InvalidCallerId,
    ] {
        assert!(!error.to_string().is_empty());
        assert!(error.source().is_none(), "{error:?} has no cause");
    }
}
