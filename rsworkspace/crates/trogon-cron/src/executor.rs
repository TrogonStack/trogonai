use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::{
    config::{Action, JobConfig, TickPayload},
    error::CronError,
};

/// Per-job runtime state tracked by the scheduler.
pub struct JobState {
    pub config: JobConfig,
    pub next_fire: Option<DateTime<Utc>>,
    pub last_fired: Option<DateTime<Utc>>,
    /// Used for `concurrent: false` spawn jobs.
    pub is_running: Arc<AtomicBool>,
    /// Parsed cron schedule (only set for Schedule::Cron jobs).
    pub parsed_cron: Option<cron::Schedule>,
}

impl JobState {
    pub fn should_fire(&self, now: DateTime<Utc>) -> bool {
        self.config.enabled && self.next_fire.map_or(false, |t| now >= t)
    }

    /// Recompute `next_fire` after a job fires or when the config is (re)loaded.
    pub fn compute_next_fire(&mut self, from: DateTime<Utc>) {
        self.next_fire = match &self.config.schedule {
            crate::config::Schedule::Interval { interval_sec } => {
                Some(from + chrono::Duration::seconds(*interval_sec as i64))
            }
            crate::config::Schedule::Cron { .. } => {
                self.parsed_cron
                    .as_ref()
                    .and_then(|s| s.after(&from).next())
            }
        };
    }
}

/// Fire a job: publish to NATS or spawn a process.
pub async fn execute(nats: &async_nats::Client, state: &JobState, now: DateTime<Utc>) {
    let tick = TickPayload {
        job_id: state.config.id.clone(),
        fired_at: now,
        execution_id: Uuid::new_v4().to_string(),
        payload: state.config.payload.clone(),
    };

    match &state.config.action {
        Action::Publish { subject } => publish(nats, subject, &tick).await,
        Action::Spawn {
            bin,
            args,
            concurrent,
            timeout_sec,
        } => {
            if !concurrent && state.is_running.load(Ordering::SeqCst) {
                tracing::debug!(job_id = %state.config.id, "Skipping tick — previous invocation still running");
                return;
            }
            spawn_process(
                bin.clone(),
                args.clone(),
                *timeout_sec,
                Arc::clone(&state.is_running),
                tick,
            );
        }
    }
}

async fn publish(nats: &async_nats::Client, subject: &str, tick: &TickPayload) {
    match serde_json::to_vec(tick) {
        Ok(payload) => {
            if let Err(e) = nats.publish(subject.to_string(), payload.into()).await {
                tracing::error!(subject, error = %e, "Failed to publish tick");
            }
        }
        Err(e) => tracing::error!(error = %e, "Failed to serialize tick payload"),
    }
}

fn spawn_process(
    bin: String,
    args: Vec<String>,
    timeout_sec: Option<u64>,
    is_running: Arc<AtomicBool>,
    tick: TickPayload,
) {
    is_running.store(true, Ordering::SeqCst);

    tokio::spawn(async move {
        tracing::debug!(bin = %bin, job_id = %tick.job_id, "Spawning process");

        let mut child = match tokio::process::Command::new(&bin).args(&args).spawn() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(bin = %bin, error = %e, "Failed to spawn process");
                is_running.store(false, Ordering::SeqCst);
                return;
            }
        };

        if let Some(secs) = timeout_sec {
            match tokio::time::timeout(Duration::from_secs(secs), child.wait()).await {
                Ok(Ok(status)) => tracing::debug!(bin = %bin, %status, "Process completed"),
                Ok(Err(e)) => tracing::error!(bin = %bin, error = %e, "Process wait error"),
                Err(_) => {
                    tracing::warn!(bin = %bin, timeout_sec = secs, "Process timed out, killing");
                    let _ = child.start_kill();
                }
            }
        } else {
            match child.wait().await {
                Ok(status) => tracing::debug!(bin = %bin, %status, "Process completed"),
                Err(e) => tracing::error!(bin = %bin, error = %e, "Process wait error"),
            }
        }

        is_running.store(false, Ordering::SeqCst);
    });
}

/// Build a `JobState` from a `JobConfig`, pre-parsing cron expressions.
pub fn build_job_state(config: JobConfig) -> Result<JobState, CronError> {
    build_job_state_at(config, Utc::now())
}

/// Build a `JobState` from a `JobConfig` at a specific time (useful for testing).
pub fn build_job_state_at(config: JobConfig, now: DateTime<Utc>) -> Result<JobState, CronError> {
    use std::str::FromStr;

    let parsed_cron = match &config.schedule {
        crate::config::Schedule::Cron { expr } => {
            let sched = cron::Schedule::from_str(expr).map_err(|e| {
                CronError::InvalidCronExpression {
                    expr: expr.clone(),
                    reason: e.to_string(),
                }
            })?;
            Some(sched)
        }
        _ => None,
    };

    let mut state = JobState {
        config,
        next_fire: None,
        last_fired: None,
        is_running: Arc::new(AtomicBool::new(false)),
        parsed_cron,
    };
    // Interval jobs fire immediately on first load; cron jobs fire at next scheduled time.
    match &state.config.schedule {
        crate::config::Schedule::Interval { .. } => {
            state.next_fire = Some(now);
        }
        crate::config::Schedule::Cron { .. } => {
            state.compute_next_fire(now);
        }
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Action, Schedule};

    fn publish_job(id: &str, interval_sec: u64) -> JobConfig {
        JobConfig {
            id: id.to_string(),
            schedule: Schedule::Interval { interval_sec },
            action: Action::Publish { subject: format!("cron.{id}") },
            enabled: true,
            payload: None,
        }
    }

    #[test]
    fn interval_job_fires_immediately_on_load() {
        let state = build_job_state(publish_job("health", 30)).unwrap();
        let now = Utc::now();
        assert!(state.should_fire(now));
    }

    #[test]
    fn interval_job_does_not_fire_before_interval() {
        let now = Utc::now();
        let mut state = build_job_state_at(publish_job("health", 30), now).unwrap();
        // Simulate the job just fired
        state.last_fired = Some(now);
        state.compute_next_fire(now);
        // 1 second later — should NOT fire yet (interval is 30s)
        let one_sec_later = now + chrono::Duration::seconds(1);
        assert!(!state.should_fire(one_sec_later));
    }

    #[test]
    fn interval_job_fires_after_interval() {
        let now = Utc::now();
        let mut state = build_job_state_at(publish_job("health", 30), now).unwrap();
        state.last_fired = Some(now);
        state.compute_next_fire(now);
        let thirty_sec_later = now + chrono::Duration::seconds(30);
        assert!(state.should_fire(thirty_sec_later));
    }

    #[test]
    fn disabled_job_never_fires() {
        let mut config = publish_job("health", 1);
        config.enabled = false;
        let state = build_job_state(config).unwrap();
        assert!(!state.should_fire(Utc::now()));
    }

    #[test]
    fn cron_job_builds_with_valid_expression() {
        let config = JobConfig {
            id: "hourly".to_string(),
            schedule: Schedule::Cron { expr: "0 0 * * * *".to_string() },
            action: Action::Publish { subject: "cron.hourly".to_string() },
            enabled: true,
            payload: None,
        };
        let state = build_job_state(config).unwrap();
        assert!(state.next_fire.is_some());
    }

    #[test]
    fn cron_job_fails_with_invalid_expression() {
        let config = JobConfig {
            id: "bad".to_string(),
            schedule: Schedule::Cron { expr: "not-a-cron".to_string() },
            action: Action::Publish { subject: "cron.bad".to_string() },
            enabled: true,
            payload: None,
        };
        assert!(build_job_state(config).is_err());
    }
}
