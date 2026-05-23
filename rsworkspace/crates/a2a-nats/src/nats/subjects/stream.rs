use async_nats::jetstream::stream::{Config, DiscardPolicy, RetentionPolicy, StorageType};

use crate::a2a_prefix::A2aPrefix;
use crate::constants::DEFAULT_STREAM_MAX_AGE;

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
            Self::PushDlq => vec![format!("{p}.push.dlq.*.*")],
        }
    }

    pub fn config(&self, prefix: &A2aPrefix) -> Config {
        Config {
            name: self.stream_name(prefix),
            subjects: self.subject_patterns(prefix),
            storage: StorageType::File,
            retention: RetentionPolicy::Limits,
            max_age: DEFAULT_STREAM_MAX_AGE,
            discard: DiscardPolicy::Old,
            ..Default::default()
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
