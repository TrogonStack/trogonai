use std::error::Error;

use super::*;

#[test]
fn display_mint() {
    assert_eq!(
        BridgeError::Mint("unavailable".into()).to_string(),
        "auth callout mint failed: unavailable"
    );
}

#[test]
fn source_for_deserialize() {
    let e = BridgeError::Deserialize(serde_json::from_str::<serde_json::Value>("]").unwrap_err());
    assert!(e.source().is_some());
}

#[test]
fn source_for_missing_authorization_none() {
    assert!(BridgeError::MissingAuthorization.source().is_none());
}

#[test]
fn a_refused_caller_credential_reads_as_a_mint_failure_with_its_cause() {
    let e = BridgeError::MintCredential(a2a_auth_callout::BridgeCredentialError::NoCalloutXkey);
    assert_eq!(
        e.to_string(),
        "auth callout mint failed: no callout xkey is configured, so no caller credential can cross the mint exchange"
    );
    assert!(e.source().is_some());
}
