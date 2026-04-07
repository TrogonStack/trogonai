//! Run history — persists automation execution records in NATS KV.
//!
//! Each record is stored under key `{tenant_id}.{run_id}` in the `RUNS` bucket.
//! The bucket has an 8-day TTL so old records are purged automatically.

use std::time::{SystemTime, UNIX_EPOCH};

use async_nats::jetstream::{self, kv};
use bytes::Bytes;
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};

use crate::store::StoreError;

/// NATS KV bucket name for run records.
pub const RUNS_BUCKET: &str = "RUNS";

/// Outcome of a single automation execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Success,
    Failed,
}

/// A persisted record of one automation execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    /// Unique run ID (UUID v4).
    pub id: String,
    /// ID of the automation that ran.
    pub automation_id: String,
    /// Human-readable automation name (snapshot at run time).
    pub automation_name: String,
    /// Tenant that owns this run.
    pub tenant_id: String,
    /// NATS subject that triggered the automation (e.g. `"github.pull_request"`).
    pub nats_subject: String,
    /// Unix timestamp (seconds) when the run started.
    pub started_at: u64,
    /// Unix timestamp (seconds) when the run finished.
    pub finished_at: u64,
    /// Whether the run succeeded or failed.
    pub status: RunStatus,
    /// Final model output (or error message on failure).
    pub output: String,
}

/// Aggregate statistics for a tenant's automations.
#[derive(Debug, Serialize)]
pub struct RunStats {
    /// Total number of stored runs (up to 8 days).
    pub total: u64,
    /// Successful runs in the last 7 days.
    pub successful_7d: u64,
    /// Failed runs in the last 7 days.
    pub failed_7d: u64,
}

/// NATS KV-backed store for [`RunRecord`]s.
#[derive(Clone)]
pub struct RunStore {
    kv: kv::Store,
}

impl RunStore {
    /// Open (or create) the `RUNS` KV bucket with an 8-day TTL.
    pub async fn open(js: &jetstream::Context) -> Result<Self, StoreError> {
        let kv = js
            .create_or_update_key_value(kv::Config {
                bucket: RUNS_BUCKET.to_string(),
                history: 1,
                max_age: std::time::Duration::from_secs(8 * 86_400),
                ..Default::default()
            })
            .await
            .map_err(|e| StoreError(e.to_string()))?;
        Ok(Self { kv })
    }

    /// Persist a run record.
    pub async fn record(&self, run: &RunRecord) -> Result<(), StoreError> {
        let key = format!("{}.{}", run.tenant_id, run.id);
        let bytes = serde_json::to_vec(run).map_err(|e| StoreError(e.to_string()))?;
        self.kv
            .put(&key, Bytes::from(bytes))
            .await
            .map_err(|e| StoreError(e.to_string()))?;
        Ok(())
    }

    /// List all runs for `tenant_id`, optionally filtered by `automation_id`.
    /// Results are sorted newest-first.
    pub async fn list(
        &self,
        tenant_id: &str,
        automation_id: Option<&str>,
    ) -> Result<Vec<RunRecord>, StoreError> {
        let prefix = format!("{tenant_id}.");
        let mut keys = self
            .kv
            .keys()
            .await
            .map_err(|e| StoreError(e.to_string()))?;
        let mut result = Vec::new();

        while let Some(key) = keys.next().await {
            let key = key.map_err(|e| StoreError(e.to_string()))?;
            if !key.starts_with(&prefix) {
                continue;
            }
            if let Some(bytes) = self
                .kv
                .get(&key)
                .await
                .map_err(|e| StoreError(e.to_string()))?
                && let Ok(run) = serde_json::from_slice::<RunRecord>(&bytes)
            {
                if let Some(aid) = automation_id
                    && run.automation_id != aid
                {
                    continue;
                }
                result.push(run);
            }
        }

        result.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        Ok(result)
    }

    /// Compute aggregate stats for `tenant_id`.
    pub async fn stats(&self, tenant_id: &str) -> Result<RunStats, StoreError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let seven_days_ago = now.saturating_sub(7 * 86_400);

        let runs = self.list(tenant_id, None).await?;
        let total = runs.len() as u64;
        let successful_7d = runs
            .iter()
            .filter(|r| r.started_at >= seven_days_ago && r.status == RunStatus::Success)
            .count() as u64;
        let failed_7d = runs
            .iter()
            .filter(|r| r.started_at >= seven_days_ago && r.status == RunStatus::Failed)
            .count() as u64;

        Ok(RunStats {
            total,
            successful_7d,
            failed_7d,
        })
    }
}

/// Return the current Unix timestamp in seconds.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_run(id: &str, status: RunStatus, started_at: u64) -> RunRecord {
        RunRecord {
            id: id.to_string(),
            automation_id: "auto-1".to_string(),
            automation_name: "My automation".to_string(),
            tenant_id: "acme".to_string(),
            nats_subject: "github.push".to_string(),
            started_at,
            finished_at: started_at + 5,
            status,
            output: "ok".to_string(),
        }
    }

    #[test]
    fn run_record_round_trips_json() {
        let r = sample_run("run-1", RunStatus::Success, 1_700_000_000);
        let json = serde_json::to_string(&r).unwrap();
        let r2: RunRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(r2.id, "run-1");
        assert_eq!(r2.status, RunStatus::Success);
    }

    #[test]
    fn run_status_serializes_as_snake_case() {
        let v = serde_json::to_value(RunStatus::Success).unwrap();
        assert_eq!(v, "success");
        let v = serde_json::to_value(RunStatus::Failed).unwrap();
        assert_eq!(v, "failed");
    }

    #[test]
    fn stats_counts_correctly() {
        let now = now_unix();
        let runs = vec![
            sample_run("r1", RunStatus::Success, now - 3600), // 1h ago — in 7d window
            sample_run("r2", RunStatus::Failed, now - 3600),  // 1h ago — in 7d window
            sample_run("r3", RunStatus::Success, now - 8 * 86_400), // 8d ago — outside window
        ];

        let seven_days_ago = now.saturating_sub(7 * 86_400);
        let total = runs.len() as u64;
        let successful_7d = runs
            .iter()
            .filter(|r| r.started_at >= seven_days_ago && r.status == RunStatus::Success)
            .count() as u64;
        let failed_7d = runs
            .iter()
            .filter(|r| r.started_at >= seven_days_ago && r.status == RunStatus::Failed)
            .count() as u64;

        assert_eq!(total, 3);
        assert_eq!(successful_7d, 1);
        assert_eq!(failed_7d, 1);
    }

    #[test]
    fn now_unix_is_reasonable() {
        let t = now_unix();
        // 2026-01-01 in unix is roughly 1_767_225_600
        assert!(t > 1_767_000_000, "timestamp looks wrong: {t}");
    }

    #[test]
    fn run_record_all_fields_accessible() {
        let r = RunRecord {
            id: "run-abc".to_string(),
            automation_id: "auto-42".to_string(),
            automation_name: "Deploy check".to_string(),
            tenant_id: "acme".to_string(),
            nats_subject: "github.push".to_string(),
            started_at: 1_700_000_000,
            finished_at: 1_700_000_010,
            status: RunStatus::Success,
            output: "All checks passed.".to_string(),
        };
        assert_eq!(r.id, "run-abc");
        assert_eq!(r.automation_id, "auto-42");
        assert_eq!(r.automation_name, "Deploy check");
        assert_eq!(r.tenant_id, "acme");
        assert_eq!(r.nats_subject, "github.push");
        assert_eq!(r.started_at, 1_700_000_000);
        assert_eq!(r.finished_at, 1_700_000_010);
        assert_eq!(r.status, RunStatus::Success);
        assert_eq!(r.output, "All checks passed.");
    }

    #[test]
    fn run_record_failed_status_round_trips() {
        let r = sample_run("run-fail", RunStatus::Failed, 1_700_000_100);
        let json = serde_json::to_string(&r).unwrap();
        let r2: RunRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(r2.status, RunStatus::Failed);
        assert_eq!(r2.id, "run-fail");
    }

    #[test]
    fn run_status_deserializes_from_snake_case() {
        let success: RunStatus = serde_json::from_str("\"success\"").unwrap();
        let failed: RunStatus = serde_json::from_str("\"failed\"").unwrap();
        assert_eq!(success, RunStatus::Success);
        assert_eq!(failed, RunStatus::Failed);
    }

    #[test]
    fn run_record_duration_computed_correctly() {
        let r = sample_run("run-dur", RunStatus::Success, 1_700_000_000);
        // finished_at is started_at + 5 per sample_run helper
        assert_eq!(r.finished_at - r.started_at, 5);
    }

    #[test]
    fn run_stats_struct_fields() {
        let stats = RunStats {
            total: 10,
            successful_7d: 7,
            failed_7d: 3,
        };
        let v: serde_json::Value = serde_json::to_value(&stats).unwrap();
        assert_eq!(v["total"], 10);
        assert_eq!(v["successful_7d"], 7);
        assert_eq!(v["failed_7d"], 3);
    }
}
