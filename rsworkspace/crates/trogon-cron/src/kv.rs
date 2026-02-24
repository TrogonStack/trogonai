use async_nats::jetstream::{self, kv, stream};
use futures::StreamExt;
use std::time::Duration;

use crate::{config::JobConfig, error::CronError};

pub const CONFIG_BUCKET: &str = "cron_configs";
pub const LEADER_BUCKET: &str = "cron_leader";
pub const LEADER_KEY: &str = "lock";
pub const JOBS_WATCH_PATTERN: &str = "jobs.*";
pub const JOBS_KEY_PREFIX: &str = "jobs.";
pub const TICKS_STREAM: &str = "CRON_TICKS";
/// Wildcard subject that captures all cron tick subjects (e.g. `cron.backup`, `cron.report`).
const TICKS_SUBJECT_PATTERN: &str = "cron.>";

pub async fn get_or_create_config_bucket(
    js: &jetstream::Context,
) -> Result<kv::Store, CronError> {
    get_or_create(
        js,
        kv::Config {
            bucket: CONFIG_BUCKET.to_string(),
            history: 5,
            ..Default::default()
        },
    )
    .await
}

pub async fn get_or_create_leader_bucket(
    js: &jetstream::Context,
) -> Result<kv::Store, CronError> {
    get_or_create(
        js,
        kv::Config {
            bucket: LEADER_BUCKET.to_string(),
            history: 1,
            // Entries older than 10s are purged — this is the leader TTL.
            max_age: Duration::from_secs(10),
            ..Default::default()
        },
    )
    .await
}

/// Ensure the `CRON_TICKS` JetStream stream exists.
///
/// The stream captures every `cron.>` subject so workers can create durable
/// consumers and receive ticks even after a brief downtime (at-least-once).
/// Ticks older than 1 hour are discarded to prevent flooding workers that
/// were down for an extended period.
pub async fn get_or_create_ticks_stream(js: &jetstream::Context) -> Result<(), CronError> {
    let config = stream::Config {
        name: TICKS_STREAM.to_string(),
        subjects: vec![TICKS_SUBJECT_PATTERN.to_string()],
        max_age: Duration::from_secs(3600),
        ..Default::default()
    };
    match js.create_stream(config).await {
        Ok(_) => Ok(()),
        Err(_) => js
            .get_stream(TICKS_STREAM)
            .await
            .map(|_| ())
            .map_err(|e| CronError::Kv(e.to_string())),
    }
}

async fn get_or_create(js: &jetstream::Context, config: kv::Config) -> Result<kv::Store, CronError> {
    let name = config.bucket.clone();
    match js.create_key_value(config).await {
        Ok(store) => Ok(store),
        Err(_) => js
            .get_key_value(&name)
            .await
            .map_err(|e| CronError::Kv(e.to_string())),
    }
}

/// Load all existing job configs from KV, then return the watcher for live updates.
/// The watcher delivers all current entries first (as Put operations), then new changes.
pub async fn load_jobs_and_watch(
    kv: &kv::Store,
) -> Result<(Vec<JobConfig>, kv::Watch), CronError> {
    // `watch_with_history` uses DeliverPolicy::LastPerSubject so the watcher
    // delivers the current value of every matching key as an initial snapshot,
    // then continues streaming live updates.  Plain `watch()` uses
    // DeliverPolicy::New and would skip all pre-existing entries.
    let mut watcher = kv
        .watch_with_history(JOBS_WATCH_PATTERN)
        .await
        .map_err(|e| CronError::Kv(e.to_string()))?;

    let mut jobs = Vec::new();

    // Drain initial snapshot. NATS delivers all current entries first;
    // `entry.delta == 0` signals the last entry in the initial batch.
    // We use a short timeout to handle the case where the bucket is empty.
    let deadline = tokio::time::sleep(Duration::from_millis(500));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            biased;
            entry = watcher.next() => {
                match entry {
                    Some(Ok(e)) => {
                        let is_last = e.delta == 0;
                        if e.operation == kv::Operation::Put {
                            if let Ok(job) = serde_json::from_slice::<JobConfig>(&e.value) {
                                jobs.push(job);
                            } else {
                                tracing::warn!(key = %e.key, "Failed to deserialize job config during initial load");
                            }
                        }
                        if is_last {
                            break;
                        }
                    }
                    Some(Err(e)) => {
                        return Err(CronError::Kv(e.to_string()));
                    }
                    None => break,
                }
            }
            _ = &mut deadline => {
                break; // bucket empty or no more entries
            }
        }
    }

    Ok((jobs, watcher))
}
