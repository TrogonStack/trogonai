use super::*;
use crate::trust::TrustRegistryError;

#[test]
fn access_server_error_wraps_verification_error_via_from() {
    let verification_err = RequestVerificationError::AgentTokenIsSubagent;
    let err: AccessServerError = verification_err.into();
    assert!(matches!(err, AccessServerError::Verification(_)));
}

#[test]
fn access_server_error_wraps_mint_error_via_from() {
    let mint_err = MintError::TtlOverflow {
        iat: i64::MAX,
        ttl_secs: 10,
    };
    let err: AccessServerError = mint_err.into();
    assert!(matches!(err, AccessServerError::Mint(_)));
}

#[test]
fn request_verification_error_untrusted_ps_preserves_source() {
    let err =
        RequestVerificationError::UntrustedPs(TrustRegistryError::UnknownIssuer("https://unknown-ps.example".into()));
    assert_eq!(
        err.to_string(),
        "untrusted PS: unknown or untrusted PS issuer: https://unknown-ps.example"
    );
}

#[test]
fn unknown_pending_request_message_includes_id() {
    let err = AccessServerError::UnknownPendingRequest("pending-123".into());
    assert_eq!(err.to_string(), "no pending claims-required request for id pending-123");
}
