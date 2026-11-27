use trogon_sandbox_contract::{LeaseError, SandboxFreezeResponse, SandboxSuspendResponse, SandboxWakeResponse};

use super::{suspend_reply, wake_reply};

fn refusal() -> LeaseError {
    LeaseError::Backend { message: "no".into() }
}

#[test]
fn a_suspend_keeps_no_artifact() {
    assert!(matches!(
        suspend_reply(SandboxFreezeResponse::Done),
        SandboxSuspendResponse::Done {
            artifact: None,
            artifact_bytes: None
        }
    ));
    assert!(matches!(
        suspend_reply(SandboxFreezeResponse::Unsupported),
        SandboxSuspendResponse::Unsupported
    ));
    assert!(matches!(
        suspend_reply(SandboxFreezeResponse::Denied(refusal())),
        SandboxSuspendResponse::Denied(_)
    ));
}

#[test]
fn a_wake_carries_the_outcome_over() {
    assert!(matches!(
        wake_reply(SandboxFreezeResponse::Done),
        SandboxWakeResponse::Done
    ));
    assert!(matches!(
        wake_reply(SandboxFreezeResponse::Unsupported),
        SandboxWakeResponse::Unsupported
    ));
    assert!(matches!(
        wake_reply(SandboxFreezeResponse::Denied(refusal())),
        SandboxWakeResponse::Denied(_)
    ));
}
