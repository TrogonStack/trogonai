use super::*;

#[test]
fn names_the_session_states() {
    assert_eq!(SessionState::Idle.as_str(), "idle");
    assert_eq!(SessionState::Running.as_str(), "running");
    assert_eq!(SessionState::RequiresAction.as_str(), "requires_action");
}

#[test]
fn names_the_failure_sources() {
    assert_eq!(FailureSource::Origin.as_str(), "origin");
    assert_eq!(FailureSource::NonoriginCf.as_str(), "nonorigin_cf");
    assert_eq!(FailureSource::NonoriginOther.as_str(), "nonorigin_other");
}

#[test]
fn describes_credentials_failures() {
    let device = CredentialsFailure::UntrustedDevice {
        refusal: DeviceRefusal::TokenRefused,
    };
    let rejected = CredentialsFailure::RequestRejected {
        status: 403,
        source: Some(FailureSource::Origin),
    };
    let plain = CredentialsFailure::RequestRejected {
        status: 404,
        source: None,
    };
    let expected = [
        (CredentialsFailure::InvalidSessionId, "invalid_session_id", None),
        (device, "untrusted_device", None),
        (CredentialsFailure::SessionStaleRelogin, "session_stale_relogin", None),
        (
            CredentialsFailure::SessionSecurityTierBelowOrgPolicy,
            "session_security_tier_below_org_policy",
            Some(403),
        ),
        (rejected.clone(), "request_rejected", Some(403)),
        (
            CredentialsFailure::MalformedResponse { status: 200 },
            "malformed_response",
            Some(200),
        ),
    ];
    for (failure, reason, status) in expected {
        assert_eq!(failure.reason(), reason);
        assert_eq!(failure.status(), status);
        assert!(failure.terminal());
    }
    assert_eq!(rejected.source(), Some(FailureSource::Origin));
    assert_eq!(plain.source(), None);
    assert_eq!(CredentialsFailure::InvalidSessionId.source(), None);
}

#[test]
fn describes_create_session_failures() {
    let grouping = CreateSessionFailure::GroupingRejected {
        status: 404,
        detail: Some("gone".to_owned()),
    };
    let rejected = CreateSessionFailure::RequestRejected {
        status: 400,
        detail: None,
    };
    let malformed = CreateSessionFailure::MalformedResponse { status: 200 };
    assert_eq!(
        (grouping.reason(), grouping.status(), grouping.detail()),
        ("grouping_rejected", 404, Some("gone"))
    );
    assert_eq!(
        (rejected.reason(), rejected.status(), rejected.detail()),
        ("request_rejected", 400, None)
    );
    assert_eq!(
        (malformed.reason(), malformed.status(), malformed.detail()),
        ("malformed_response", 200, None)
    );
    assert!(grouping.terminal() && rejected.terminal() && malformed.terminal());
}

#[test]
fn classifies_outcomes() {
    let rejection = CredentialsRejection;
    assert_eq!((rejection.reason(), rejection.terminal()), ("oauth_rejected", false));
    let failure = RemoteCredentialsOutcome::Failure(CredentialsFailure::InvalidSessionId);
    assert!(failure.is_credentials_failure() && !failure.is_credentials_rejection());
    let rejected = RemoteCredentialsOutcome::Rejected(rejection);
    assert!(rejected.is_credentials_rejection() && !rejected.is_credentials_failure());
    let failure = CreateSessionOutcome::Failure(CreateSessionFailure::MalformedResponse { status: 1 });
    assert!(failure.is_create_session_failure() && !failure.is_credentials_rejection());
    let rejected = CreateSessionOutcome::Rejected(rejection);
    assert!(rejected.is_credentials_rejection() && !rejected.is_create_session_failure());
    assert!(!CreateSessionOutcome::Unavailable.is_create_session_failure());
}
