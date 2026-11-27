use trogon_sandbox_contract::{
    LeaseError, LeaseId, SandboxFreezeResponse, SandboxSuspendResponse, SandboxWakeResponse,
};

use super::{suspended, woken};

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
