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
mod tests;
