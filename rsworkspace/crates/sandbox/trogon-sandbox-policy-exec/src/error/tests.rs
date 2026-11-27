use super::*;
#[cfg(target_os = "linux")]
use trogon_sandbox_subprocess_worker::isolation::mount_plan::FsPlanError;
#[cfg(target_os = "macos")]
use trogon_sandbox_subprocess_worker::isolation::seatbelt::sbpl::SbplError;

#[test]
fn a_launch_failure_without_a_profile_cause_stays_a_launch_error() {
    let error = RuntimeError::launch(anyhow::anyhow!("fork failed"));
    assert!(matches!(error, RuntimeError::Launch(_)), "{error}");
}

#[cfg(target_os = "macos")]
#[test]
fn a_profile_the_backend_cannot_express_is_a_profile_error() {
    let cause = anyhow::Error::new(SbplError::UnsupportedGlob("a[/]b".to_string()));
    let error = RuntimeError::launch(cause.context("building the profile"));
    assert!(matches!(error, RuntimeError::Profile(_)), "{error}");
}

#[cfg(target_os = "linux")]
#[test]
fn a_mount_plan_the_backend_cannot_express_is_a_profile_error() {
    let cause = anyhow::Error::new(FsPlanError::TooManyArguments {
        total: 99999,
        mandatory: 0,
        configured: 99999,
    });
    let error = RuntimeError::launch(cause.context("planning the mounts"));
    assert!(matches!(error, RuntimeError::Profile(_)), "{error}");
}
