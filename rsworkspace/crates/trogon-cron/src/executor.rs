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

/// Validate an `Action::Spawn` config before accepting the job.
///
/// Checks performed:
/// - `bin` must be an absolute path
/// - `bin` must exist and be executable (Unix only)
/// - No argument may contain a null byte
fn validate_spawn_config(bin: &str, args: &[String]) -> Result<(), CronError> {
    let path = std::path::Path::new(bin);
    if !path.is_absolute() {
        return Err(CronError::InvalidJobConfig {
            reason: format!("bin must be an absolute path, got: {bin}"),
        });
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(path).map_err(|e| CronError::InvalidJobConfig {
            reason: format!("cannot access bin '{bin}': {e}"),
        })?;
        if meta.permissions().mode() & 0o111 == 0 {
            return Err(CronError::InvalidJobConfig {
                reason: format!("bin '{bin}' is not executable"),
            });
        }
    }

    for arg in args {
        if arg.contains('\0') {
            return Err(CronError::InvalidJobConfig {
                reason: format!("argument contains null byte: {arg:?}"),
            });
        }
    }

    Ok(())
}

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
        self.config.enabled && self.next_fire.is_some_and(|t| now >= t)
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

        // Build the command and inject tick context as environment variables so
        // the spawned binary knows which job fired, when, and with what payload.
        let mut cmd = tokio::process::Command::new(&bin);
        cmd.args(&args)
            .env("CRON_JOB_ID", &tick.job_id)
            .env("CRON_FIRED_AT", tick.fired_at.to_rfc3339())
            .env("CRON_EXECUTION_ID", &tick.execution_id)
            // If the scheduler process is dropped (crash, SIGKILL) the OS will
            // receive the Child handle drop and send SIGKILL to the child.
            .kill_on_drop(true);
        if let Some(ref payload) = tick.payload {
            if let Ok(json) = serde_json::to_string(payload) {
                cmd.env("CRON_PAYLOAD", json);
            }
        }

        let mut child = match cmd.spawn() {
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
                Err(_) => kill_gracefully(&mut child, &bin, secs).await,
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

/// Send SIGTERM and wait up to 5 s for a clean exit; escalate to SIGKILL if needed.
///
/// On non-Unix platforms SIGTERM is not available, so we go straight to SIGKILL.
async fn kill_gracefully(child: &mut tokio::process::Child, bin: &str, timeout_sec: u64) {
    tracing::warn!(bin, timeout_sec, "Process timed out, sending SIGTERM");

    #[cfg(unix)]
    if let Some(pid) = child.id() {
        use nix::sys::signal::{Signal, kill as nix_kill};
        use nix::unistd::Pid;
        let _ = nix_kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
        match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
            Ok(_) => return, // exited gracefully after SIGTERM
            Err(_) => tracing::warn!(bin, "SIGTERM ignored, escalating to SIGKILL"),
        }
    }

    // SIGKILL fallback (also the only path on non-Unix).
    let _ = child.start_kill();
    match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::error!(bin, error = %e, "Error waiting for killed process"),
        Err(_) => tracing::error!(bin, "Process still alive 5 s after SIGKILL"),
    }
}

/// Build a `JobState` from a `JobConfig`, pre-parsing cron expressions.
pub fn build_job_state(config: JobConfig) -> Result<JobState, CronError> {
    build_job_state_at(config, Utc::now())
}

/// Build a `JobState` from a `JobConfig` at a specific time (useful for testing).
pub fn build_job_state_at(config: JobConfig, now: DateTime<Utc>) -> Result<JobState, CronError> {
    use std::str::FromStr;

    // Validate schedule parameters.
    if let crate::config::Schedule::Interval { interval_sec } = &config.schedule {
        if *interval_sec == 0 {
            return Err(CronError::InvalidJobConfig {
                reason: "interval_sec must be >= 1".into(),
            });
        }
        if *interval_sec > i64::MAX as u64 {
            return Err(CronError::InvalidJobConfig {
                reason: "interval_sec is too large".into(),
            });
        }
    }

    // Validate publish config.
    if let Action::Publish { subject } = &config.action {
        if !subject.starts_with("cron.") {
            return Err(CronError::InvalidJobConfig {
                reason: format!(
                    "publish subject must start with 'cron.', got: {subject}"
                ),
            });
        }
    }

    // Validate spawn config.
    if let Action::Spawn { bin, args, timeout_sec, .. } = &config.action {
        validate_spawn_config(bin, args)?;
        if timeout_sec.is_some_and(|s| s == 0) {
            return Err(CronError::InvalidJobConfig {
                reason: "timeout_sec must be >= 1 when set".into(),
            });
        }
    }

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

    #[test]
    fn interval_sec_zero_is_rejected() {
        let config = JobConfig {
            id: "zero".to_string(),
            schedule: Schedule::Interval { interval_sec: 0 },
            action: Action::Publish { subject: "cron.zero".to_string() },
            enabled: true,
            payload: None,
        };
        let err = build_job_state(config).err().unwrap();
        assert!(err.to_string().contains("interval_sec"));
    }

    #[test]
    fn timeout_sec_zero_is_rejected() {
        let config = JobConfig {
            id: "timeout-zero".to_string(),
            schedule: Schedule::Interval { interval_sec: 60 },
            action: Action::Spawn {
                bin: "/usr/bin/true".to_string(),
                args: vec![],
                concurrent: false,
                timeout_sec: Some(0),
            },
            enabled: true,
            payload: None,
        };
        let err = build_job_state(config).err().unwrap();
        assert!(err.to_string().contains("timeout_sec"));
    }

    #[test]
    fn publish_subject_without_cron_prefix_is_rejected() {
        let config = JobConfig {
            id: "bad-subject".to_string(),
            schedule: Schedule::Interval { interval_sec: 60 },
            action: Action::Publish { subject: "events.backup".to_string() },
            enabled: true,
            payload: None,
        };
        let err = build_job_state(config).err().unwrap();
        assert!(err.to_string().contains("cron."));
    }

    #[test]
    fn publish_subject_with_cron_prefix_is_accepted() {
        let config = JobConfig {
            id: "good-subject".to_string(),
            schedule: Schedule::Interval { interval_sec: 60 },
            action: Action::Publish { subject: "cron.backup".to_string() },
            enabled: true,
            payload: None,
        };
        assert!(build_job_state(config).is_ok());
    }
}
