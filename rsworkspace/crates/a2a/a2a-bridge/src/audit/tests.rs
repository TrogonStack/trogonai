use crate::edge_identity::CredentialSubjectError;

use super::*;

fn minted() -> SubjectPrincipal {
    SubjectPrincipal::from_tokens("person", "harness").unwrap()
}

fn signer() -> CredentialSubject {
    CredentialSubject::new("https://agents.test", "aauth:agent-1@agents.test").unwrap()
}

#[test]
fn a_record_names_the_bridge_as_the_presenter_whoever_the_caller_is() {
    for caller in [
        RecordedCaller::SubjectPrincipal(minted()),
        RecordedCaller::EdgeIdentity(signer()),
        RecordedCaller::Unverified,
    ] {
        let record = BridgedRequestRecord::new(caller.clone(), ReqId::from_header("req-1"));
        assert_eq!(record.presenter(), "a2a-bridge");
        assert_eq!(record.caller(), &caller);
        assert_eq!(record.req_id().as_str(), "req-1");
    }
}

#[test]
fn an_edge_failure_vouches_only_for_a_signer_whose_bearer_names_somebody_else() {
    let mismatch = EdgeAuthError::BearerMismatch {
        verified: signer(),
        bearer: CredentialSubject::new("https://agents.test", "aauth:agent-2@agents.test").unwrap(),
    };
    assert_eq!(
        RecordedCaller::behind(&mismatch),
        RecordedCaller::EdgeIdentity(signer())
    );

    let unidentified = EdgeAuthError::UnidentifiedPresenter(CredentialSubjectError::EmptySubject);
    assert_eq!(RecordedCaller::behind(&unidentified), RecordedCaller::Unverified);
}

#[test]
fn a_caller_reads_as_who_it_is_and_how_the_bridge_knows_it() {
    for (caller, shown, source) in [
        (
            RecordedCaller::SubjectPrincipal(minted()),
            "person.harness",
            "subject_principal",
        ),
        (
            RecordedCaller::EdgeIdentity(signer()),
            "aauth:agent-1@agents.test (https://agents.test)",
            "edge_identity",
        ),
        (RecordedCaller::Unverified, "unverified", "unverified"),
    ] {
        assert_eq!(caller.to_string(), shown);
        assert_eq!(caller.source(), source);
    }
}
