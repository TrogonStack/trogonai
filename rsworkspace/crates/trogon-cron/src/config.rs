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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_interval_roundtrip() {
        let s = Schedule::Interval { interval_sec: 30 };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"type\":\"interval\""));
        assert!(json.contains("\"interval_sec\":30"));
        let s2: Schedule = serde_json::from_str(&json).unwrap();
        assert!(matches!(s2, Schedule::Interval { interval_sec: 30 }));
    }

    #[test]
    fn schedule_cron_roundtrip() {
        let s = Schedule::Cron { expr: "0 */5 * * * *".to_string() };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"type\":\"cron\""));
        let s2: Schedule = serde_json::from_str(&json).unwrap();
        assert!(matches!(s2, Schedule::Cron { .. }));
    }

    #[test]
    fn action_publish_roundtrip() {
        let a = Action::Publish { subject: "cron.backup".to_string() };
        let json = serde_json::to_string(&a).unwrap();
        assert!(json.contains("\"type\":\"publish\""));
        let a2: Action = serde_json::from_str(&json).unwrap();
        assert!(matches!(a2, Action::Publish { .. }));
    }

    #[test]
    fn action_spawn_roundtrip() {
        let a = Action::Spawn {
            bin: "/usr/bin/backup".to_string(),
            args: vec!["--mode".to_string(), "full".to_string()],
            concurrent: false,
            timeout_sec: Some(60),
        };
        let json = serde_json::to_string(&a).unwrap();
        assert!(json.contains("\"type\":\"spawn\""));
        let a2: Action = serde_json::from_str(&json).unwrap();
        assert!(matches!(a2, Action::Spawn { .. }));
    }

    #[test]
    fn job_config_enabled_defaults_to_true() {
        let json = r#"{
            "id": "test",
            "schedule": {"type": "interval", "interval_sec": 10},
            "action": {"type": "publish", "subject": "test"}
        }"#;
        let config: JobConfig = serde_json::from_str(json).unwrap();
        assert!(config.enabled);
    }

    #[test]
    fn job_config_full_roundtrip() {
        let config = JobConfig {
            id: "backup".to_string(),
            schedule: Schedule::Interval { interval_sec: 3600 },
            action: Action::Publish { subject: "cron.backup".to_string() },
            enabled: true,
            payload: Some(serde_json::json!({ "db": "main" })),
        };
        let json = serde_json::to_string(&config).unwrap();
        let config2: JobConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config2.id, "backup");
        assert!(config2.enabled);
    }
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
