use async_nats::jetstream::stream::Config;

use crate::a2a_prefix::A2aPrefix;
use crate::nats::subjects::A2aStream;

use super::stream_options::StreamProvisionOptions;

pub fn events_stream_name(prefix: &A2aPrefix) -> String {
    A2aStream::Events.stream_name(prefix)
}

pub fn all_configs(prefix: &A2aPrefix) -> [Config; 2] {
    A2aStream::all_configs(prefix)
}

pub fn all_configs_with_options(prefix: &A2aPrefix, options: &StreamProvisionOptions) -> [Config; 2] {
    super::stream_options::all_configs_with_options(prefix, options)
}

#[cfg(test)]
mod tests {
    use async_nats::jetstream::stream::{DiscardPolicy, RetentionPolicy, StorageType};

    use super::*;
    use crate::constants::DEFAULT_STREAM_MAX_AGE;

    fn p(s: &str) -> A2aPrefix {
        A2aPrefix::new(s.to_string()).expect("test prefix")
    }

    #[test]
    fn events_stream_name_formats_correctly() {
        assert_eq!(events_stream_name(&p("a2a")), "A2A_EVENTS");
        assert_eq!(events_stream_name(&p("myapp")), "MYAPP_EVENTS");
    }

    #[test]
    fn events_stream_normalizes_dotted_prefix() {
        assert_eq!(events_stream_name(&p("vendor.a2a")), "VENDOR_A2A_EVENTS");
    }

    #[test]
    fn events_subjects_cover_all_tasks() {
        let config = A2aStream::Events.config(&p("a2a"));
        assert_eq!(config.subjects, vec!["a2a.task.*.events.*"]);
    }

    #[test]
    fn events_stream_uses_file_storage() {
        let config = A2aStream::Events.config(&p("a2a"));
        assert_eq!(config.storage, StorageType::File);
    }

    #[test]
    fn events_stream_has_max_age() {
        let config = A2aStream::Events.config(&p("a2a"));
        assert_eq!(config.max_age, DEFAULT_STREAM_MAX_AGE);
    }

    #[test]
    fn events_stream_uses_discard_old() {
        let config = A2aStream::Events.config(&p("a2a"));
        assert_eq!(config.discard, DiscardPolicy::Old);
    }

    #[test]
    fn events_stream_uses_interest_retention() {
        let config = A2aStream::Events.config(&p("a2a"));
        assert_eq!(config.retention, RetentionPolicy::Interest);
    }

    #[test]
    fn all_configs_returns_both_streams() {
        let configs = all_configs(&p("a2a"));
        assert_eq!(configs.len(), 2);
        let names: Vec<String> = configs.iter().map(|c| c.name.clone()).collect();
        assert!(names.contains(&"A2A_EVENTS".to_string()));
        assert!(names.contains(&"A2A_PUSH_DLQ".to_string()));
    }

    #[test]
    fn push_dlq_subjects_cover_caller_and_task() {
        let config = A2aStream::PushDlq.config(&p("a2a"));
        assert_eq!(config.subjects, vec!["a2a.push.dlq.*.*"]);
    }

    #[test]
    fn push_dlq_stream_uses_file_storage() {
        let config = A2aStream::PushDlq.config(&p("a2a"));
        assert_eq!(config.storage, StorageType::File);
    }

    #[test]
    fn push_dlq_stream_has_max_age() {
        let config = A2aStream::PushDlq.config(&p("a2a"));
        assert_eq!(config.max_age, DEFAULT_STREAM_MAX_AGE);
    }

    #[test]
    fn push_dlq_stream_uses_discard_old() {
        let config = A2aStream::PushDlq.config(&p("a2a"));
        assert_eq!(config.discard, DiscardPolicy::Old);
    }

    #[test]
    fn push_dlq_stream_uses_limits_retention() {
        let config = A2aStream::PushDlq.config(&p("a2a"));
        assert_eq!(config.retention, RetentionPolicy::Limits);
    }
}
