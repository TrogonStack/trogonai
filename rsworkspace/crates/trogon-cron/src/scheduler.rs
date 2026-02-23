use std::collections::HashMap;
use std::time::Duration;

use async_nats::jetstream;
use chrono::Utc;
use futures::StreamExt;
use uuid::Uuid;

use crate::{
    error::CronError,
    executor::{JobState, build_job_state, execute},
    kv::{self, get_or_create_config_bucket, get_or_create_leader_bucket, load_jobs_and_watch},
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

        // 1. Get/create KV buckets.
        let config_kv = get_or_create_config_bucket(&js).await?;
        let leader_kv = get_or_create_leader_bucket(&js).await?;

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
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("Shutdown signal received");
                    leader.release().await;
                    break;
                }

                _ = tick.tick() => {
                    if leader.ensure_leader().await {
                        let now = Utc::now();
                        for state in jobs.values_mut() {
                            if state.should_fire(now) {
                                execute(&self.nats, state, now).await;
                                state.last_fired = Some(now);
                                state.compute_next_fire(now);
                            }
                        }
                    }
                }

                entry = config_watcher.next() => {
                    match entry {
                        Some(Ok(e)) => handle_config_change(&mut jobs, e),
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
                    Ok(state) => {
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
