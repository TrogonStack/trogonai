use trogon_sandbox_windows_fence::{ProfileReadState, WfpResidual, WfpState};

#[test]
fn state_names_are_stable() {
    for (state, name) in [
        (ProfileReadState::None, "none"),
        (ProfileReadState::Current, "current"),
        (ProfileReadState::Outdated, "outdated"),
        (ProfileReadState::Incomplete, "incomplete"),
        (ProfileReadState::Unknown, "unknown"),
    ] {
        assert_eq!(state.as_str(), name);
    }
    for (state, name) in [
        (WfpState::Absent, "absent"),
        (WfpState::Installed, "installed"),
        (WfpState::LoopbackMismatch, "loopback-mismatch"),
        (WfpState::CannotRead, "cannot-read"),
    ] {
        assert_eq!(state.as_str(), name);
    }
}

#[test]
fn every_residual_has_a_name_and_a_sentence() {
    for (residual, name) in [
        (WfpResidual::AnonymousEgress, "anonymous-egress"),
        (WfpResidual::AnonymousLoopback, "anonymous-loopback"),
        (WfpResidual::DenyOnlyUserSid, "deny-only-user-sid"),
        (WfpResidual::LoweredIntegrityUnprobed, "lowered-integrity-unprobed"),
    ] {
        assert_eq!(residual.as_str(), name);
        assert!(!residual.text().is_empty());
    }
}
