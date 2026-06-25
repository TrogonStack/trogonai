use super::*;

#[cfg(feature = "test-support")]
mod with_mocks;

#[test]
fn log_on_error_does_nothing_for_published() {
    let outcome: PublishOutcome<String> = PublishOutcome::Published;
    outcome.log_on_error("test");
}

#[test]
fn is_ok_reports_only_published() {
    assert!(PublishOutcome::<String>::Published.is_ok());
    assert!(!PublishOutcome::<String>::PublishFailed("fail".into()).is_ok());
    assert!(!PublishOutcome::<String>::AckFailed("ack".into()).is_ok());
    assert!(!PublishOutcome::<String>::AckTimedOut(Duration::from_secs(1)).is_ok());
}

#[test]
fn log_on_error_logs_publish_and_ack_failures() {
    PublishOutcome::<String>::PublishFailed("publish".into()).log_on_error("test");
    PublishOutcome::<String>::AckFailed("ack".into()).log_on_error("test");
    PublishOutcome::<String>::AckTimedOut(Duration::from_millis(50)).log_on_error("test");
}

#[test]
fn log_on_error_logs_store_failed() {
    let err: Box<dyn std::error::Error + Send + Sync> = "store down".into();
    let outcome: PublishOutcome<String> = PublishOutcome::StoreFailed(err);
    outcome.log_on_error("test");
}
