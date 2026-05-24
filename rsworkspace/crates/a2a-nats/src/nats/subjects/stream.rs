use async_nats::jetstream::stream::{Config, DiscardPolicy, RetentionPolicy, StorageType};

use crate::a2a_prefix::A2aPrefix;
use crate::constants::{DEFAULT_PUSH_DLQ_DEDUP_WINDOW_SECS, DEFAULT_STREAM_MAX_AGE};
use crate::jetstream::stream_options::EventsStreamMaxAge;

/// The JetStream stream that captures a subject's messages.
///
/// A2A only persists task event streams and push DLQ envelopes — request/reply over
/// Core NATS doesn't need JetStream replay. The `Events` stream backs `message/stream`
/// delivery and `tasks/resubscribe` reconnect-after-disconnect. The `PushDlq` stream
/// captures terminal push delivery failures for operator remediation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum A2aStream {
    Events,
    PushDlq,
}

impl A2aStream {
    pub const ALL: [A2aStream; 2] = [Self::Events, Self::PushDlq];

    pub fn suffix(&self) -> &'static str {
        match self {
            Self::Events => "EVENTS",
            Self::PushDlq => "PUSH_DLQ",
        }
    }

    pub fn stream_name(&self, prefix: &A2aPrefix) -> String {
        format!("{}_{}", prefix.as_str().to_uppercase().replace('.', "_"), self.suffix())
    }

    pub fn subject_patterns(&self, prefix: &A2aPrefix) -> Vec<String> {
        let p = prefix.as_str();
        match self {
            Self::Events => vec![format!("{p}.task.*.events.*")],
            Self::PushDlq => vec![
                format!("{p}.push.dlq.*.*"),
                format!("{p}.push.dlq.mirror.*.*"),
            ],
        }
    }

    pub fn events_config(prefix: &A2aPrefix, max_age: EventsStreamMaxAge) -> Config {
        Config {
            name: Self::Events.stream_name(prefix),
            subjects: Self::Events.subject_patterns(prefix),
            storage: StorageType::File,
            retention: RetentionPolicy::Interest,
            max_age: max_age.as_duration(),
            discard: DiscardPolicy::Old,
            ..Default::default()
        }
    }

    pub fn push_dlq_config(prefix: &A2aPrefix, duplicate_window: std::time::Duration) -> Config {
        Config {
            name: Self::PushDlq.stream_name(prefix),
            subjects: Self::PushDlq.subject_patterns(prefix),
            storage: StorageType::File,
            retention: RetentionPolicy::Limits,
            max_age: DEFAULT_STREAM_MAX_AGE,
            discard: DiscardPolicy::Old,
            duplicate_window,
            ..Default::default()
        }
    }

    pub fn config(&self, prefix: &A2aPrefix) -> Config {
        match self {
            Self::Events => Self::events_config(prefix, EventsStreamMaxAge::DEFAULT),
            Self::PushDlq => Self::push_dlq_config(
                prefix,
                std::time::Duration::from_secs(DEFAULT_PUSH_DLQ_DEDUP_WINDOW_SECS),
            ),
        }
    }

    pub fn all_configs(prefix: &A2aPrefix) -> [Config; 2] {
        Self::ALL.map(|s| s.config(prefix))
    }
}

impl std::fmt::Display for A2aStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.suffix())
    }
}

/// A subject knows which stream captures it (if any).
pub trait StreamAssignment {
    const STREAM: Option<A2aStream>;
}
