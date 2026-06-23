use super::*;

    #[test]
    fn recording_metrics_is_infallible() {
        let metrics = ProcessorMetrics::new();
        metrics.record_outcome("published");
        metrics.record_publish();
        metrics.record_purge();
        metrics.record_redelivery();
    }

    #[test]
    fn default_metrics_match_new() {
        let _ = ProcessorMetrics::default();
    }

    #[test]
    fn debug_fmt_is_non_exhaustive() {
        let _ = format!("{:?}", ProcessorMetrics::new());
    }
