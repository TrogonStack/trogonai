use trogon_sandbox_contract::{
    LeaseError, LeaseId, SandboxFreezeResponse, SandboxSuspendResponse, SandboxWakeResponse,
};

use super::{suspended, unsupported, woken};

#[test]
fn a_suspend_reply_follows_the_freeze_outcome() {
    assert!(matches!(
        suspended(SandboxFreezeResponse::Done),
        SandboxSuspendResponse::Done { .. }
    ));
    assert!(matches!(
        suspended(SandboxFreezeResponse::Unsupported),
        SandboxSuspendResponse::Unsupported
    ));
    let denied = SandboxFreezeResponse::Denied(LeaseError::LeaseNotFound {
        lease_id: LeaseId::new("lease-1"),
    });
    assert!(matches!(suspended(denied), SandboxSuspendResponse::Denied(_)));
}

#[test]
fn a_wake_reply_follows_the_freeze_outcome() {
    assert!(matches!(woken(SandboxFreezeResponse::Done), SandboxWakeResponse::Done));
    assert!(matches!(
        woken(SandboxFreezeResponse::Unsupported),
        SandboxWakeResponse::Unsupported
    ));
}

#[test]
fn a_wake_reply_carries_a_denial() {
    let denied = SandboxFreezeResponse::Denied(LeaseError::LeaseNotFound {
        lease_id: LeaseId::new("lease-1"),
    });
    assert!(matches!(woken(denied), SandboxWakeResponse::Denied(_)));
}

#[test]
fn an_unsupported_verb_names_what_it_refuses() {
    assert!(matches!(
        unsupported("sandbox users"),
        LeaseError::Backend { message } if message == "sandbox users are not supported on the E2B provider"
    ));
}
