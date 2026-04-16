//! Run history — persists automation execution records in NATS KV.
//!
//! Each record is stored under key `{tenant_id}.{run_id}` in the `RUNS` bucket.
//! The bucket has an 8-day TTL so old records are purged automatically.

use std::future::Future;
use std::pin::Pin;
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

// ── Repository trait ──────────────────────────────────────────────────────────

/// Abstraction over a run-history store.
pub trait RunRepository: Clone + Send + Sync + 'static {
    fn record<'a>(
        &'a self,
        run: &'a RunRecord,
    ) -> Pin<Box<dyn Future<Output = Result<(), StoreError>> + Send + 'a>>;

    fn list<'a>(
        &'a self,
        tenant_id: &'a str,
        automation_id: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<RunRecord>, StoreError>> + Send + 'a>>;

    fn stats<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<RunStats, StoreError>> + Send + 'a>>;
}

impl RunRepository for RunStore {
    fn record<'a>(
        &'a self,
        run: &'a RunRecord,
    ) -> Pin<Box<dyn Future<Output = Result<(), StoreError>> + Send + 'a>> {
        Box::pin(async move { self.record(run).await })
    }

    fn list<'a>(
        &'a self,
        tenant_id: &'a str,
        automation_id: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<RunRecord>, StoreError>> + Send + 'a>> {
        Box::pin(async move { self.list(tenant_id, automation_id).await })
    }

    fn stats<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<RunStats, StoreError>> + Send + 'a>> {
        Box::pin(async move { self.stats(tenant_id).await })
    }
}

// ── In-memory mock (test-only) ────────────────────────────────────────────────

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Vec-backed in-memory run store for unit tests.
    #[derive(Clone, Default)]
    pub struct MockRunStore {
        data: Arc<Mutex<Vec<RunRecord>>>,
    }

    impl MockRunStore {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn snapshot(&self) -> Vec<RunRecord> {
            self.data.lock().unwrap().clone()
        }

        /// Pre-populate the store with a run record (test helper).
        pub fn insert(&self, run: RunRecord) {
            self.data.lock().unwrap().push(run);
        }
    }

    /// Run store that always returns errors — used to test 500 paths.
    #[derive(Clone, Default)]
    pub struct ErrorRunStore;

    impl ErrorRunStore {
        pub fn new() -> Self {
            Self
        }
    }

    impl RunRepository for ErrorRunStore {
        fn record<'a>(
            &'a self,
            _run: &'a RunRecord,
        ) -> Pin<Box<dyn Future<Output = Result<(), StoreError>> + Send + 'a>> {
            Box::pin(async move { Err(StoreError("injected record error".into())) })
        }

        fn list<'a>(
            &'a self,
            _tenant_id: &'a str,
            _automation_id: Option<&'a str>,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<RunRecord>, StoreError>> + Send + 'a>> {
            Box::pin(async move { Err(StoreError("injected list error".into())) })
        }

        fn stats<'a>(
            &'a self,
            _tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<RunStats, StoreError>> + Send + 'a>> {
            Box::pin(async move { Err(StoreError("injected stats error".into())) })
        }
    }

    impl RunRepository for MockRunStore {
        fn record<'a>(
            &'a self,
            run: &'a RunRecord,
        ) -> Pin<Box<dyn Future<Output = Result<(), StoreError>> + Send + 'a>> {
            let data = Arc::clone(&self.data);
            let run = run.clone();
            Box::pin(async move {
                data.lock().unwrap().push(run);
                Ok(())
            })
        }

        fn list<'a>(
            &'a self,
            tenant_id: &'a str,
            automation_id: Option<&'a str>,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<RunRecord>, StoreError>> + Send + 'a>> {
            let data = Arc::clone(&self.data);
            let tenant_id = tenant_id.to_string();
            let automation_id = automation_id.map(str::to_string);
            Box::pin(async move {
                let guard = data.lock().unwrap();
                let mut runs: Vec<RunRecord> = guard
                    .iter()
                    .filter(|r| r.tenant_id == tenant_id)
                    .filter(|r| {
                        automation_id
                            .as_deref()
                            .is_none_or(|aid| r.automation_id == aid)
                    })
                    .cloned()
                    .collect();
                runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
                Ok(runs)
            })
        }

        fn stats<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<RunStats, StoreError>> + Send + 'a>> {
            let data = Arc::clone(&self.data);
            let tenant_id = tenant_id.to_string();
            Box::pin(async move {
                let now = now_unix();
                let seven_days_ago = now.saturating_sub(7 * 86_400);
                let guard = data.lock().unwrap();
                let runs: Vec<&RunRecord> =
                    guard.iter().filter(|r| r.tenant_id == tenant_id).collect();
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
            })
        }
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
        let runs = [
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

    /// The 7-day window filter uses `>= seven_days_ago`.  A record at exactly
    /// the boundary must be counted; one second before must not.
    #[test]
    fn stats_boundary_records_counted_at_boundary_excluded_just_outside() {
        let now = now_unix();
        let seven_days_ago = now.saturating_sub(7 * 86_400);

        let runs = [
            sample_run("r1", RunStatus::Success, seven_days_ago),     // exactly at boundary — in
            sample_run("r2", RunStatus::Failed, seven_days_ago - 1),  // 1 s before — out
            sample_run("r3", RunStatus::Success, now),                 // now — in
        ];

        let successful_7d = runs
            .iter()
            .filter(|r| r.started_at >= seven_days_ago && r.status == RunStatus::Success)
            .count() as u64;
        let failed_7d = runs
            .iter()
            .filter(|r| r.started_at >= seven_days_ago && r.status == RunStatus::Failed)
            .count() as u64;

        assert_eq!(successful_7d, 2, "r1 (boundary) and r3 (now) must be counted");
        assert_eq!(failed_7d, 0, "r2 (just before boundary) must not be counted");
        assert_eq!(runs.len() as u64, 3, "total includes all records regardless of age");
    }

    /// When all runs are older than 7 days the 7d counters must be zero but
    /// `total` still reflects the full history.
    #[test]
    fn stats_all_outside_7d_window_gives_zero_7d_counts() {
        let now = now_unix();
        let eight_days_ago = now.saturating_sub(8 * 86_400);
        let seven_days_ago = now.saturating_sub(7 * 86_400);

        let runs = [
            sample_run("r1", RunStatus::Success, eight_days_ago),
            sample_run("r2", RunStatus::Failed, eight_days_ago),
        ];

        let successful_7d = runs
            .iter()
            .filter(|r| r.started_at >= seven_days_ago && r.status == RunStatus::Success)
            .count() as u64;
        let failed_7d = runs
            .iter()
            .filter(|r| r.started_at >= seven_days_ago && r.status == RunStatus::Failed)
            .count() as u64;

        assert_eq!(successful_7d, 0);
        assert_eq!(failed_7d, 0);
        assert_eq!(runs.len() as u64, 2, "total must still count older runs");
    }

    /// Concurrent `record()` calls on `MockRunStore` must all land without
    /// data races — the `Arc<Mutex<Vec<...>>>` must serialise concurrent inserts.
    #[tokio::test]
    async fn mock_run_store_concurrent_records_are_safe() {
        use super::mock::MockRunStore;
        use super::RunRepository as _;

        let store = std::sync::Arc::new(MockRunStore::new());
        let now = now_unix();

        let handles: Vec<_> = (0..20u64)
            .map(|i| {
                let s = std::sync::Arc::clone(&store);
                let run = RunRecord {
                    id: format!("run-{i}"),
                    automation_id: "auto-1".to_string(),
                    automation_name: "concurrent test".to_string(),
                    tenant_id: "acme".to_string(),
                    nats_subject: "github.push".to_string(),
                    started_at: now + i,
                    finished_at: now + i + 1,
                    status: if i % 2 == 0 {
                        RunStatus::Success
                    } else {
                        RunStatus::Failed
                    },
                    output: format!("out-{i}"),
                };
                tokio::spawn(async move { s.record(&run).await })
            })
            .collect();

        for h in handles {
            h.await.expect("task must not panic").expect("record must succeed");
        }

        let all = store.list("acme", None).await.unwrap();
        assert_eq!(all.len(), 20, "all 20 concurrent records must be present");

        // Half successful, half failed
        let successes = all.iter().filter(|r| r.status == RunStatus::Success).count();
        let failures = all.iter().filter(|r| r.status == RunStatus::Failed).count();
        assert_eq!(successes, 10);
        assert_eq!(failures, 10);
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
