use tracing::Level;

use super::*;
use trogon_std::log_capture::{CapturedEvents, LevelFilter};

#[test]
fn dry_run_warns_that_denials_are_not_enforced() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::INFO);

    enforcement_notice(EnforcementMode::DryRun);

    let warnings = captured.at(Level::WARN);
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].field("mode"), Some("dry_run"));
    assert!(
        warnings[0]
            .message()
            .is_some_and(|message| message.contains("not enforced"))
    );
}

#[test]
fn enforcement_adds_no_warning() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::INFO);

    enforcement_notice(EnforcementMode::Enforce);

    assert!(captured.at(Level::WARN).is_empty());
}
