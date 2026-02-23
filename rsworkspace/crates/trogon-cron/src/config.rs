use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// How often a job fires.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Schedule {
    /// Fire every N seconds (minimum 1).
    Interval { interval_sec: u64 },
    /// Unix-style 6-field cron expression: "0 */5 * * * *" (sec min hour dom month dow).
    Cron { expr: String },
}

/// What happens when a job fires.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Publish a NATS message to this subject.
    Publish { subject: String },
    /// Spawn a process directly.
    Spawn {
        bin: String,
        #[serde(default)]
        args: Vec<String>,
        /// If false (default), skip the tick if the previous invocation is still running.
        #[serde(default)]
        concurrent: bool,
        /// Kill the process after this many seconds.
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout_sec: Option<u64>,
    },
}

/// Job definition stored in NATS KV under key `jobs.<id>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobConfig {
    pub id: String,
    pub schedule: Schedule,
    pub action: Action,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Extra data forwarded in every tick payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

fn default_true() -> bool {
    true
}

/// Message published to NATS when a job fires.
#[derive(Debug, Serialize, Deserialize)]
pub struct TickPayload {
    pub job_id: String,
    pub fired_at: DateTime<Utc>,
    /// Unique ID per execution — use for deduplication on the consumer side.
    pub execution_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}
