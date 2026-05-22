use async_nats::jetstream::stream::{Config, DiscardPolicy, RetentionPolicy, StorageType};

use crate::a2a_prefix::A2aPrefix;
use crate::constants::DEFAULT_STREAM_MAX_AGE;

/// The JetStream stream that captures a subject's messages.
///
/// A2A only persists task event streams — request/reply over Core NATS doesn't need
/// JetStream replay. The `Events` stream backs `message/stream` delivery and
/// `tasks/resubscribe` reconnect-after-disconnect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum A2aStream {
    Events,
}

impl A2aStream {
    pub const ALL: [A2aStream; 1] = [Self::Events];

    pub fn suffix(&self) -> &'static str {
        match self {
            Self::Events => "EVENTS",
        }
    }

    pub fn stream_name(&self, prefix: &A2aPrefix) -> String {
        format!("{}_{}", prefix.as_str().to_uppercase().replace('.', "_"), self.suffix())
    }

    pub fn subject_patterns(&self, prefix: &A2aPrefix) -> Vec<String> {
        let p = prefix.as_str();
        match self {
            Self::Events => vec![format!("{p}.task.*.events.*")],
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

    pub fn all_configs(prefix: &A2aPrefix) -> [Config; 1] {
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
