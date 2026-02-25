use std::collections::HashMap;
use std::time::Duration;

use async_nats::jetstream;
use chrono::Utc;
use futures::StreamExt;
use uuid::Uuid;

use crate::{
    error::CronError,
    executor::{JobState, build_job_state, execute},
    kv::{self, get_or_create_config_bucket, get_or_create_leader_bucket,
         get_or_create_ticks_stream, load_jobs_and_watch},
    leader::LeaderElection,
    nats_impls::NatsLeaderLock,
};

const TICK_INTERVAL: Duration = Duration::from_millis(500);

pub struct Scheduler {
    nats: async_nats::Client,
    node_id: String,
}

impl Scheduler {
    pub fn new(nats: async_nats::Client) -> Self {
        Self {
            nats,
            node_id: Uuid::new_v4().to_string(),
        }
    }

    /// Run the scheduler until SIGTERM / Ctrl-C.
    pub async fn run(self) -> Result<(), CronError> {
        let js = jetstream::new(self.nats.clone());

        // 1. Get/create KV buckets and tick stream.
        let config_kv = get_or_create_config_bucket(&js).await?;
        let leader_kv = get_or_create_leader_bucket(&js).await?;
        get_or_create_ticks_stream(&js).await?;

        // 2. Activate KV watch BEFORE loading jobs to avoid missing changes.
        let (initial_jobs, mut config_watcher) = load_jobs_and_watch(&config_kv).await?;

        tracing::info!(
            node_id = %self.node_id,
            job_count = initial_jobs.len(),
            "CRON scheduler starting"
        );

        // 3. Build job state map.
        let mut jobs: HashMap<String, JobState> = HashMap::new();
        for config in initial_jobs {
            match build_job_state(config) {
                Ok(state) => {
                    jobs.insert(state.config.id.clone(), state);
                }
                Err(e) => tracing::error!(error = %e, "Skipping invalid job config"),
            }
        }

        // 4. Leader election.
        let mut leader = LeaderElection::new(NatsLeaderLock::new(leader_kv), self.node_id.clone());

        // 5. Main loop.
        let mut tick = tokio::time::interval(TICK_INTERVAL);

        loop {
            tokio::select! {
                _ = shutdown_signal() => {
                    tracing::info!("Shutdown signal received, releasing leader lock");
                    leader.release().await;
                    break;
                }

                _ = tick.tick() => {
                    if leader.ensure_leader().await {
                        let now = Utc::now();
                        for state in jobs.values_mut() {
                            if state.should_fire(now) {
                                execute(&js, state, now).await;
                                state.last_fired = Some(now);
                                state.compute_next_fire(now);
                            }
                        }
                    }
                }

                entry = config_watcher.next() => {
                    match entry {
                        Some(Ok(e)) => {
                            handle_config_change(&mut jobs, e);
                        }
                        Some(Err(e)) => tracing::error!(error = %e, "Config watcher error"),
                        None => {
                            tracing::warn!("Config watcher stream ended");
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

/// Resolves when the process receives a shutdown signal (SIGINT or SIGTERM).
///
/// On Unix both signals are handled so container orchestrators (`docker stop`,
/// Kubernetes pod termination) trigger a clean leader-lock release.
/// On non-Unix only Ctrl-C (SIGINT) is available.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let sigterm = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let sigterm = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c  => {}
        _ = sigterm => {}
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::Ordering;

    use async_nats::jetstream::kv;
    use bytes::Bytes;

    use super::*;
    use crate::{
        config::{Action, JobConfig, Schedule},
        executor::build_job_state,
    };

    fn make_entry(key: &str, operation: kv::Operation, value: Bytes) -> kv::Entry {
        kv::Entry {
            bucket: "cron_configs".to_string(),
            key: key.to_string(),
            value,
            revision: 1,
            delta: 0,
            created: time::OffsetDateTime::UNIX_EPOCH,
            seen_current: false,
            operation,
        }
    }

    fn insert_job(jobs: &mut HashMap<String, crate::executor::JobState>, id: &str) {
        let config = JobConfig {
            id: id.to_string(),
            schedule: Schedule::Interval { interval_sec: 60 },
            action: Action::Publish { subject: format!("cron.{id}") },
            enabled: true,
            payload: None,
            retry: None,
        };
        jobs.insert(id.to_string(), build_job_state(config).unwrap());
    }

    fn job_config_json(id: &str) -> Bytes {
        let config = JobConfig {
            id: id.to_string(),
            schedule: Schedule::Interval { interval_sec: 60 },
            action: Action::Publish { subject: format!("cron.{id}") },
            enabled: true,
            payload: None,
            retry: None,
        };
        Bytes::from(serde_json::to_vec(&config).unwrap())
    }

    // ── handle_config_change ──────────────────────────────────────────────────

    #[test]
    fn put_adds_new_job() {
        let mut jobs = HashMap::new();
        let entry = make_entry("jobs.fresh", kv::Operation::Put, job_config_json("fresh"));
        handle_config_change(&mut jobs, entry);
        assert!(jobs.contains_key("fresh"), "Put must insert the job");
        assert_eq!(jobs["fresh"].config.id, "fresh");
    }

    #[test]
    fn delete_existing_job_removes_it() {
        let mut jobs = HashMap::new();
        insert_job(&mut jobs, "gone");
        let entry = make_entry("jobs.gone", kv::Operation::Delete, Bytes::new());
        handle_config_change(&mut jobs, entry);
        assert!(!jobs.contains_key("gone"), "Delete must remove the job");
    }

    #[test]
    fn delete_nonexistent_job_is_noop() {
        let mut jobs = HashMap::new();
        let entry = make_entry("jobs.ghost", kv::Operation::Delete, Bytes::new());
        handle_config_change(&mut jobs, entry);
        assert!(jobs.is_empty(), "Delete of non-existent job must not insert anything");
    }

    #[test]
    fn purge_removes_job() {
        let mut jobs = HashMap::new();
        insert_job(&mut jobs, "purgeable");
        let entry = make_entry("jobs.purgeable", kv::Operation::Purge, Bytes::new());
        handle_config_change(&mut jobs, entry);
        assert!(!jobs.contains_key("purgeable"), "Purge must remove the job");
    }

    #[test]
    fn malformed_json_does_not_insert_job() {
        let mut jobs = HashMap::new();
        let entry = make_entry(
            "jobs.bad",
            kv::Operation::Put,
            Bytes::from(b"{not valid json}".to_vec()),
        );
        handle_config_change(&mut jobs, entry);
        assert!(jobs.is_empty(), "Malformed JSON must not insert a job");
    }

    /// When a job's config is hot-reloaded via Put, the new `JobState` must
    /// share the same `is_running` Arc so any in-flight spawn remains visible.
    #[test]
    fn put_preserves_is_running_from_old_state() {
        let mut jobs = HashMap::new();
        insert_job(&mut jobs, "live");

        // Simulate the job currently executing.
        jobs["live"].is_running.store(true, Ordering::SeqCst);
        let old_arc = std::sync::Arc::clone(&jobs["live"].is_running);

        // Hot-reload with a new config (changed interval).
        let updated = JobConfig {
            id: "live".to_string(),
            schedule: Schedule::Interval { interval_sec: 120 },
            action: Action::Publish { subject: "cron.live".to_string() },
            enabled: true,
            payload: None,
            retry: None,
        };
        let entry = make_entry(
            "jobs.live",
            kv::Operation::Put,
            Bytes::from(serde_json::to_vec(&updated).unwrap()),
        );
        handle_config_change(&mut jobs, entry);

        let new_arc = &jobs["live"].is_running;
        assert!(
            std::sync::Arc::ptr_eq(new_arc, &old_arc),
            "hot-reload must preserve is_running Arc"
        );
        assert!(
            new_arc.load(Ordering::SeqCst),
            "is_running flag must be preserved as true after hot-reload"
        );
    }

    /// Key without the "jobs." prefix falls back to using the full key as the job id.
    #[test]
    fn key_without_prefix_uses_full_key() {
        let mut jobs = HashMap::new();
        // Construct a config whose id matches the raw key (no prefix).
        let config = JobConfig {
            id: "raw-key".to_string(),
            schedule: Schedule::Interval { interval_sec: 60 },
            action: Action::Publish { subject: "cron.raw-key".to_string() },
            enabled: true,
            payload: None,
            retry: None,
        };
        let value = Bytes::from(serde_json::to_vec(&config).unwrap());
        // Entry key has NO "jobs." prefix — strip_prefix fails, falls back to full key.
        let entry = make_entry("raw-key", kv::Operation::Put, value);
        handle_config_change(&mut jobs, entry);
        assert!(
            jobs.contains_key("raw-key"),
            "job must be inserted under the full key when prefix is absent"
        );
    }
}

fn handle_config_change(
    jobs: &mut HashMap<String, JobState>,
    entry: async_nats::jetstream::kv::Entry,
) {
    use async_nats::jetstream::kv::Operation;

    let id = entry
        .key
        .strip_prefix(kv::JOBS_KEY_PREFIX)
        .unwrap_or(&entry.key)
        .to_string();

    match entry.operation {
        Operation::Put => {
            match serde_json::from_slice(&entry.value) {
                Ok(config) => match build_job_state(config) {
                    Ok(mut state) => {
                        // Preserve is_running from the old state so any in-flight spawned
                        // process remains visible to the scheduler after a hot-reload.
                        if let Some(old) = jobs.get(&id) {
                            state.is_running = std::sync::Arc::clone(&old.is_running);
                        }
                        tracing::info!(job_id = %id, "Job config updated");
                        jobs.insert(id, state);
                    }
                    Err(e) => tracing::error!(job_id = %id, error = %e, "Invalid job config, skipping"),
                },
                Err(e) => tracing::error!(job_id = %id, error = %e, "Failed to deserialize job config"),
            }
        }
        Operation::Delete | Operation::Purge => {
            if jobs.remove(&id).is_some() {
                tracing::info!(job_id = %id, "Job removed");
            }
        }
    }
}
