use std::error::Error;

use trogon_aauth_verify::ReplayError;

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

fn subject(subject: &str) -> CredentialSubject {
    CredentialSubject::new("https://agents.test", subject).unwrap()
}

#[test]
fn a_verifier_failure_is_sorted_by_what_the_caller_has_to_do() {
    assert!(matches!(
        EdgeAuthError::from_pop(HttpPopError::MissingHeader("signature")),
        EdgeAuthError::Unsigned(HttpPopError::MissingHeader("signature"))
    ));
    assert!(matches!(
        EdgeAuthError::from_pop(HttpPopError::BadSignature),
        EdgeAuthError::Unverified(HttpPopError::BadSignature)
    ));
    assert!(matches!(
        EdgeAuthError::from_pop(HttpPopError::Replay),
        EdgeAuthError::Unverified(HttpPopError::Replay)
    ));
    assert!(matches!(
        EdgeAuthError::from_pop(HttpPopError::Backend(ReplayError::MutexPoisoned)),
        EdgeAuthError::Unavailable(HttpPopError::Backend(ReplayError::MutexPoisoned))
    ));
}

#[test]
fn only_a_missing_proof_or_an_unnamed_bearer_leaves_a_request_anonymous() {
    assert!(EdgeAuthError::Unsigned(HttpPopError::MissingHeader("signature")).leaves_request_anonymous());
    assert!(EdgeAuthError::UnidentifiedBearer(BearerSubjectError::NotBearer).leaves_request_anonymous());

    for named in [
        EdgeAuthError::Unverified(HttpPopError::BadSignature),
        EdgeAuthError::Unavailable(HttpPopError::Backend(ReplayError::MutexPoisoned)),
        EdgeAuthError::BearerMismatch {
            verified: subject("aauth:agent-1@agents.test"),
            bearer: subject("aauth:agent-2@agents.test"),
        },
        EdgeAuthError::UnidentifiedPresenter(CredentialSubjectError::EmptySubject),
    ] {
        assert!(!named.leaves_request_anonymous(), "{named:?}");
    }
}

#[test]
fn the_detail_names_both_callers_of_a_mismatch_and_every_cause() {
    let mismatch = EdgeAuthError::BearerMismatch {
        verified: subject("aauth:agent-1@agents.test"),
        bearer: subject("aauth:agent-2@agents.test"),
    };
    assert_eq!(
        mismatch.detail(),
        "bearer credential does not name the caller that signed the request: verified aauth:agent-1@agents.test \
         (https://agents.test), bearer aauth:agent-2@agents.test (https://agents.test)"
    );

    let unavailable = EdgeAuthError::Unavailable(HttpPopError::Backend(ReplayError::MutexPoisoned));
    assert_eq!(
        unavailable.to_string(),
        "AAuth proof of possession could not be checked"
    );
    assert_eq!(
        unavailable.detail(),
        "AAuth proof of possession could not be checked: replay store backend failure: replay store mutex poisoned"
    );
}
