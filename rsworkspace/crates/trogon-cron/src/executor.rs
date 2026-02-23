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
    traits::TickPublisher,
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

/// Fire a job: publish to NATS (via `TickPublisher`) or spawn a process.
pub async fn execute<P: TickPublisher>(publisher: &P, state: &JobState, now: DateTime<Utc>) {
    let tick = TickPayload {
        job_id: state.config.id.clone(),
        fired_at: now,
        execution_id: Uuid::new_v4().to_string(),
        payload: state.config.payload.clone(),
    };

    match &state.config.action {
        Action::Publish { subject } => publish(publisher, subject, &tick).await,
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

async fn publish<P: TickPublisher>(publisher: &P, subject: &str, tick: &TickPayload) {
    let payload = match serde_json::to_vec(tick) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "Failed to serialize tick payload");
            return;
        }
    };
    // Inject OpenTelemetry trace context so consumers can correlate ticks with traces.
    let headers = trogon_nats::messaging::headers_with_trace_context();
    if let Err(e) = publisher
        .publish_tick(subject.to_string(), headers, payload.into())
        .await
    {
        tracing::error!(subject, error = %e, "Failed to publish tick");
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

    // ── execute() tests ───────────────────────────────────────────────────────

    #[cfg(feature = "test-support")]
    mod execute_tests {
        use std::sync::atomic::Ordering;

        use super::*;
        use crate::mocks::MockTickPublisher;

        fn publish_job_state(id: &str) -> JobState {
            build_job_state(JobConfig {
                id: id.to_string(),
                schedule: Schedule::Interval { interval_sec: 60 },
                action: Action::Publish { subject: format!("cron.{id}") },
                enabled: true,
                payload: None,
            })
            .unwrap()
        }

        #[tokio::test]
        async fn publishes_one_tick_per_call() {
            let publisher = MockTickPublisher::new();
            let state = publish_job_state("smoke");

            execute(&publisher, &state, Utc::now()).await;

            assert_eq!(publisher.tick_count(), 1);
            assert_eq!(publisher.ticks()[0].subject, "cron.smoke");
        }

        #[tokio::test]
        async fn tick_payload_contains_correct_job_id() {
            let publisher = MockTickPublisher::new();
            let state = publish_job_state("heartbeat");

            execute(&publisher, &state, Utc::now()).await;

            let tick: crate::config::TickPayload =
                serde_json::from_slice(&publisher.ticks()[0].payload).unwrap();
            assert_eq!(tick.job_id, "heartbeat");
            assert!(!tick.execution_id.is_empty());
        }

        #[tokio::test]
        async fn each_call_gets_unique_execution_id() {
            let publisher = MockTickPublisher::new();
            let state = publish_job_state("report");
            let now = Utc::now();

            execute(&publisher, &state, now).await;
            execute(&publisher, &state, now).await;

            let ticks = publisher.ticks();
            let t1: crate::config::TickPayload = serde_json::from_slice(&ticks[0].payload).unwrap();
            let t2: crate::config::TickPayload = serde_json::from_slice(&ticks[1].payload).unwrap();
            assert_ne!(t1.execution_id, t2.execution_id);
        }

        #[tokio::test]
        async fn spawn_skips_when_already_running() {
            let publisher = MockTickPublisher::new();
            let state = build_job_state(JobConfig {
                id: "proc".to_string(),
                schedule: Schedule::Interval { interval_sec: 60 },
                action: Action::Spawn {
                    bin: "/usr/bin/true".to_string(),
                    args: vec![],
                    concurrent: false,
                    timeout_sec: None,
                },
                enabled: true,
                payload: None,
            })
            .unwrap();

            // Simulate a previous invocation still running.
            state.is_running.store(true, Ordering::SeqCst);
            execute(&publisher, &state, Utc::now()).await;

            // execute() returned early — is_running was not cleared.
            assert!(state.is_running.load(Ordering::SeqCst));
        }
    }

    // ── scheduling logic tests ────────────────────────────────────────────────

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
