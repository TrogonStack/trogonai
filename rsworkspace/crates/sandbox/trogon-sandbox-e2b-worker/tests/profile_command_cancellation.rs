//! The worker serves exact command cancellation but does not advertise cancellation of a launch whose start reply was
//! lost. The SDK stack acts on the advertised profile, so a client that needs launch cancellation is refused before
//! any sandbox is created, rather than being given a launch it could not cancel.

use trogon_sandbox_e2b_worker::PROFILE;

#[test]
fn only_exact_command_cancellation_is_advertised() {
    assert!(PROFILE.isolation.supports_command_cancellation.is_supported());
    assert!(
        !PROFILE.isolation.supports_command_launch_cancellation.is_supported(),
        "advertising launch cancellation changes what the SDK stack launches, so it needs a composed acceptance run"
    );
}
