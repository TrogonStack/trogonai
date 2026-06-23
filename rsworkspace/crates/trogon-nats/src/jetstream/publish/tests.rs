    use super::*;

    #[cfg(feature = "test-support")]
    mod with_mocks;

    #[test]
    fn log_on_error_does_nothing_for_published() {
        let outcome: PublishOutcome<String> = PublishOutcome::Published;
        outcome.log_on_error("test");
    }

    #[test]
    fn log_on_error_logs_store_failed() {
        let err: Box<dyn std::error::Error + Send + Sync> = "store down".into();
        let outcome: PublishOutcome<String> = PublishOutcome::StoreFailed(err);
        outcome.log_on_error("test");
    }

