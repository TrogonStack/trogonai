use std::time::Duration;

use trogon_std::env::ReadEnv;

use crate::a2a_prefix::A2aPrefix;
use crate::constants::{
    DEFAULT_PUSH_DLQ_DEDUP_WINDOW_SECS, DEFAULT_STREAM_MAX_AGE, ENV_EVENTS_MAX_AGE_SECS, ENV_PUSH_DLQ_DEDUP_WINDOW_SECS,
};
use crate::nats::subjects::A2aStream;

/// Per-Account replay window override for the **`A2A_EVENTS`** stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventsStreamMaxAge(Duration);

impl EventsStreamMaxAge {
    pub const DEFAULT: Self = Self(DEFAULT_STREAM_MAX_AGE);

    pub fn new(duration: Duration) -> Self {
        Self(duration)
    }

    pub fn from_secs(secs: u64) -> Self {
        Self(Duration::from_secs(secs.max(1)))
    }

    pub fn as_duration(self) -> Duration {
        self.0
    }
}

/// JetStream duplicate suppression window for **`A2A_PUSH_DLQ`** publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushDlqDuplicateWindow(Duration);

impl PushDlqDuplicateWindow {
    pub const DEFAULT: Self = Self(Duration::from_secs(DEFAULT_PUSH_DLQ_DEDUP_WINDOW_SECS));

    pub fn from_secs(secs: u64) -> Self {
        Self(Duration::from_secs(secs.max(1)))
    }

    pub fn as_duration(self) -> Duration {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamProvisionOptions {
    pub events_max_age: EventsStreamMaxAge,
    pub push_dlq_duplicate_window: PushDlqDuplicateWindow,
}

impl Default for StreamProvisionOptions {
    fn default() -> Self {
        Self {
            events_max_age: EventsStreamMaxAge::DEFAULT,
            push_dlq_duplicate_window: PushDlqDuplicateWindow::DEFAULT,
        }
    }
}

impl StreamProvisionOptions {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        Self {
            events_max_age: events_stream_max_age_from_env(env),
            push_dlq_duplicate_window: push_dlq_duplicate_window_from_env(env),
        }
    }
}

pub fn events_stream_max_age_from_env<E: ReadEnv>(env: &E) -> EventsStreamMaxAge {
    env.var(ENV_EVENTS_MAX_AGE_SECS)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .map(EventsStreamMaxAge::from_secs)
        .unwrap_or(EventsStreamMaxAge::DEFAULT)
}

pub fn push_dlq_duplicate_window_from_env<E: ReadEnv>(env: &E) -> PushDlqDuplicateWindow {
    env.var(ENV_PUSH_DLQ_DEDUP_WINDOW_SECS)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .map(PushDlqDuplicateWindow::from_secs)
        .unwrap_or(PushDlqDuplicateWindow::DEFAULT)
}

pub fn all_configs_with_options(
    prefix: &A2aPrefix,
    options: &StreamProvisionOptions,
) -> [async_nats::jetstream::stream::Config; 2] {
    [
        A2aStream::events_config(prefix, options.events_max_age),
        A2aStream::push_dlq_config(prefix, options.push_dlq_duplicate_window.as_duration()),
    ]
}

#[cfg(test)]
mod tests {
    use async_nats::jetstream::stream::{DiscardPolicy, RetentionPolicy};

    use super::*;
    use crate::constants::DEFAULT_STREAM_MAX_AGE;
    use trogon_std::env::InMemoryEnv;

    fn p(s: &str) -> crate::A2aPrefix {
        crate::A2aPrefix::new(s.to_string()).expect("test prefix")
    }

    #[test]
    fn events_config_uses_interest_retention_and_discard_old() {
        let config = A2aStream::events_config(&p("a2a"), EventsStreamMaxAge::DEFAULT);
        assert_eq!(config.retention, RetentionPolicy::Interest);
        assert_eq!(config.discard, DiscardPolicy::Old);
        assert_eq!(config.max_age, DEFAULT_STREAM_MAX_AGE);
        assert_eq!(config.subjects, vec!["a2a.tasks.*.events.*"]);
    }

    #[test]
    fn events_max_age_override_applies_to_config() {
        let max_age = EventsStreamMaxAge::from_secs(86_400);
        let config = A2aStream::events_config(&p("a2a"), max_age);
        assert_eq!(config.max_age, Duration::from_secs(86_400));
    }

    #[test]
    fn events_max_age_from_env_reads_override() {
        let env = InMemoryEnv::new();
        env.set(ENV_EVENTS_MAX_AGE_SECS, "3600");
        assert_eq!(
            events_stream_max_age_from_env(&env).as_duration(),
            Duration::from_secs(3600)
        );
    }

    #[test]
    fn events_max_age_new_round_trips_duration() {
        let max_age = EventsStreamMaxAge::new(Duration::from_secs(900));
        assert_eq!(max_age.as_duration(), Duration::from_secs(900));
    }

    #[test]
    fn push_dlq_duplicate_window_from_secs_clamps_to_at_least_one_second() {
        assert_eq!(
            PushDlqDuplicateWindow::from_secs(0).as_duration(),
            Duration::from_secs(1)
        );
        assert_eq!(
            PushDlqDuplicateWindow::from_secs(45).as_duration(),
            Duration::from_secs(45)
        );
    }

    #[test]
    fn push_dlq_duplicate_window_from_env_reads_override() {
        let env = InMemoryEnv::new();
        env.set(ENV_PUSH_DLQ_DEDUP_WINDOW_SECS, "300");
        assert_eq!(
            push_dlq_duplicate_window_from_env(&env).as_duration(),
            Duration::from_secs(300)
        );
    }

    #[test]
    fn push_dlq_duplicate_window_from_env_falls_back_to_default_when_unset() {
        let env = InMemoryEnv::new();
        assert_eq!(
            push_dlq_duplicate_window_from_env(&env).as_duration(),
            PushDlqDuplicateWindow::DEFAULT.as_duration()
        );
    }

    #[test]
    fn push_dlq_duplicate_window_from_env_ignores_non_numeric() {
        let env = InMemoryEnv::new();
        env.set(ENV_PUSH_DLQ_DEDUP_WINDOW_SECS, "not-a-number");
        assert_eq!(
            push_dlq_duplicate_window_from_env(&env).as_duration(),
            PushDlqDuplicateWindow::DEFAULT.as_duration()
        );
    }

    #[test]
    fn stream_provision_options_from_env_aggregates_both_knobs() {
        let env = InMemoryEnv::new();
        env.set(ENV_EVENTS_MAX_AGE_SECS, "7200");
        env.set(ENV_PUSH_DLQ_DEDUP_WINDOW_SECS, "180");
        let options = StreamProvisionOptions::from_env(&env);
        assert_eq!(options.events_max_age.as_duration(), Duration::from_secs(7200));
        assert_eq!(
            options.push_dlq_duplicate_window.as_duration(),
            Duration::from_secs(180)
        );
    }
}
