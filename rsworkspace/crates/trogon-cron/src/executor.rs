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

/// Compute exponential backoff duration (deterministic, used in unit tests).
///
/// Returns `base_sec * 2^(attempt-1)`, capped at `base_sec * 32` (five doublings max).
/// `attempt` starts at 1 for the first retry.
fn retry_backoff(base_sec: u64, attempt: u32) -> Duration {
    // 2^(attempt-1), clamped to 5 doublings → maximum multiplier is 32
    let exponent = (attempt - 1).min(5);
    let multiplier = 1u64 << exponent; // 1, 2, 4, 8, 16, 32
    Duration::from_secs(base_sec * multiplier)
}

/// Exponential backoff with ±20 % random jitter.
///
/// Jitter prevents thundering-herd: when many jobs fail simultaneously (e.g.
/// NATS restart), their retries are spread across a window instead of all
/// firing at exactly the same instant.
fn retry_backoff_with_jitter(base_sec: u64, attempt: u32, max_backoff: Option<Duration>) -> Duration {
    let base = retry_backoff(base_sec, attempt);
    // factor ∈ [0.8, 1.2) → ±20 % spread
    let factor = 0.8 + rand::random::<f64>() * 0.4;
    let delay = Duration::from_millis((base.as_millis() as f64 * factor) as u64);
    // Apply explicit ceiling after jitter so the cap is strictly respected.
    if let Some(max) = max_backoff {
        delay.min(max)
    } else {
        delay
    }
}

/// Dead-letter payload published to `cron.errors` after all retries are exhausted.
#[derive(serde::Serialize)]
struct DeadLetterPayload<'a> {
    job_id: &'a str,
    execution_id: &'a str,
    fired_at: DateTime<Utc>,
    action: &'a str,
    attempts: u32,
    error: String,
}

/// Publish a dead-letter message to `cron.errors` when a job action permanently fails.
async fn publish_dead_letter<P: TickPublisher>(
    publisher: &P,
    tick: &TickPayload,
    action: &str,
    attempts: u32,
    error: String,
) {
    let dl = DeadLetterPayload {
        job_id: &tick.job_id,
        execution_id: &tick.execution_id,
        fired_at: tick.fired_at,
        action,
        attempts,
        error,
    };
    let payload = match serde_json::to_vec(&dl) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(
                job_id = %tick.job_id,
                error = %e,
                "Failed to serialize dead-letter payload"
            );
            return;
        }
    };
    let headers = trogon_nats::messaging::headers_with_trace_context();
    if let Err(e) = publisher
        .publish_tick("cron.errors".to_string(), headers, payload.into())
        .await
    {
        tracing::error!(
            job_id = %tick.job_id,
            error = %e,
            "Failed to publish dead-letter to cron.errors"
        );
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

    let (max_retries, retry_backoff_sec, max_retry_duration, max_backoff) = state
        .config
        .retry
        .as_ref()
        .map(|r| (
            r.max_retries,
            r.retry_backoff_sec.max(1),
            r.max_retry_duration_sec.map(Duration::from_secs),
            r.max_backoff_sec.map(Duration::from_secs),
        ))
        .unwrap_or((0, 1, None, None));

    match &state.config.action {
        Action::Publish { subject } => {
            // Spawn in a background task so publish retries do not block the
            // scheduler loop or delay leader-heartbeat renewal.
            let publisher = publisher.clone();
            let subject = subject.clone();
            tokio::spawn(async move {
                publish(publisher, subject, tick, max_retries, retry_backoff_sec, max_retry_duration, max_backoff).await;
            });
        }
        Action::Spawn {
            bin,
            args,
            concurrent,
            timeout_sec,
        } => {
            let is_run = state.is_running.load(Ordering::SeqCst);
            if !concurrent && is_run {
                tracing::debug!(job_id = %state.config.id, "Skipping tick — previous invocation still running");
                return;
            }
            spawn_process(
                bin.clone(),
                args.clone(),
                *timeout_sec,
                Arc::clone(&state.is_running),
                tick,
                max_retries,
                retry_backoff_sec,
                max_retry_duration,
                max_backoff,
                publisher.clone(),
            );
        }
    }
}

async fn publish<P: TickPublisher>(
    publisher: P,
    subject: String,
    tick: TickPayload,
    max_retries: u32,
    retry_backoff_sec: u64,
    max_retry_duration: Option<Duration>,
    max_backoff: Option<Duration>,
) {
    let payload = match serde_json::to_vec(&tick) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "Failed to serialize tick payload");
            return;
        }
    };

    let total_attempts = max_retries + 1;
    let mut last_error = String::new();
    let mut actual_attempts = 0u32;
    let start = tokio::time::Instant::now();

    for attempt in 1..=total_attempts {
        actual_attempts = attempt;
        // Inject OpenTelemetry trace context so consumers can correlate ticks with traces.
        let headers = trogon_nats::messaging::headers_with_trace_context();
        match publisher
            .publish_tick(subject.clone(), headers, payload.clone().into())
            .await
        {
            Ok(()) => return,
            Err(e) => {
                last_error = e.to_string();
                if attempt < total_attempts {
                    // Stop retrying if the total duration budget is exhausted.
                    if let Some(max_dur) = max_retry_duration {
                        if start.elapsed() >= max_dur {
                            tracing::warn!(
                                subject,
                                attempt,
                                elapsed_ms = start.elapsed().as_millis(),
                                "Max retry duration exceeded, giving up"
                            );
                            break;
                        }
                    }
                    let delay = retry_backoff_with_jitter(retry_backoff_sec, attempt, max_backoff);
                    // Cap the sleep so we don't overshoot the duration budget.
                    let delay = if let Some(max_dur) = max_retry_duration {
                        delay.min(max_dur.saturating_sub(start.elapsed()))
                    } else {
                        delay
                    };
                    tracing::warn!(
                        subject,
                        attempt,
                        max_retries,
                        delay_ms = delay.as_millis(),
                        error = %e,
                        "Publish failed, retrying"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    tracing::error!(
        subject,
        attempts = total_attempts,
        error = %last_error,
        "Publish failed after all retries"
    );
    if max_retries > 0 {
        publish_dead_letter(&publisher, &tick, "publish", actual_attempts, last_error).await;
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_process<P: TickPublisher>(
    bin: String,
    args: Vec<String>,
    timeout_sec: Option<u64>,
    is_running: Arc<AtomicBool>,
    tick: TickPayload,
    max_retries: u32,
    retry_backoff_sec: u64,
    max_retry_duration: Option<Duration>,
    max_backoff: Option<Duration>,
    publisher: P,
) {
    // Set is_running BEFORE entering the async task so the concurrent-guard in
    // execute() sees the flag raised even if the OS scheduler hasn't started the
    // spawned task yet.
    is_running.store(true, Ordering::SeqCst);

    tokio::spawn(async move {
        let total_attempts = max_retries + 1;
        let mut succeeded = false;
        let mut last_error = String::new();
        let mut actual_attempts = 0u32;
        let start = tokio::time::Instant::now();

        'retry: for attempt in 0..total_attempts {
            actual_attempts += 1;
            // Delay before retry attempts (not before the first attempt).
            if attempt > 0 {
                // Stop retrying if the total duration budget is exhausted.
                if let Some(max_dur) = max_retry_duration {
                    if start.elapsed() >= max_dur {
                        tracing::warn!(
                            bin = %bin,
                            job_id = %tick.job_id,
                            attempt,
                            elapsed_ms = start.elapsed().as_millis(),
                            "Max retry duration exceeded, giving up"
                        );
                        break 'retry;
                    }
                }
                let delay = retry_backoff_with_jitter(retry_backoff_sec, attempt, max_backoff);
                // Cap the sleep so we don't overshoot the duration budget.
                let delay = if let Some(max_dur) = max_retry_duration {
                    delay.min(max_dur.saturating_sub(start.elapsed()))
                } else {
                    delay
                };
                tracing::warn!(
                    bin = %bin,
                    job_id = %tick.job_id,
                    attempt,
                    max_retries,
                    delay_ms = delay.as_millis(),
                    "Spawn failed, retrying"
                );
                tokio::time::sleep(delay).await;
            } else {
                tracing::debug!(bin = %bin, job_id = %tick.job_id, "Spawning process");
            }

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
                    // Permanent failure — the binary cannot be started at all (e.g.,
                    // ENOENT, EACCES).  Retrying won't help, so break immediately.
                    last_error = e.to_string();
                    tracing::error!(
                        bin = %bin,
                        error = %e,
                        "Failed to spawn process (permanent failure, not retrying)"
                    );
                    break 'retry;
                }
            };

            let exit_result = if let Some(secs) = timeout_sec {
                match tokio::time::timeout(Duration::from_secs(secs), child.wait()).await {
                    Ok(Ok(status)) => {
                        if status.success() {
                            tracing::debug!(bin = %bin, %status, "Process completed");
                            Ok(())
                        } else {
                            let msg = format!("Process exited with status: {status}");
                            tracing::warn!(bin = %bin, %status, "Process exited with non-zero status");
                            Err(msg)
                        }
                    }
                    Ok(Err(e)) => {
                        tracing::error!(bin = %bin, error = %e, "Process wait error");
                        Err(e.to_string())
                    }
                    Err(_) => {
                        kill_gracefully(&mut child, &bin, secs).await;
                        Err(format!("Process timed out after {secs}s"))
                    }
                }
            } else {
                match child.wait().await {
                    Ok(status) => {
                        if status.success() {
                            tracing::debug!(bin = %bin, %status, "Process completed");
                            Ok(())
                        } else {
                            let msg = format!("Process exited with status: {status}");
                            tracing::warn!(bin = %bin, %status, "Process exited with non-zero status");
                            Err(msg)
                        }
                    }
                    Err(e) => {
                        tracing::error!(bin = %bin, error = %e, "Process wait error");
                        Err(e.to_string())
                    }
                }
            };

            match exit_result {
                Ok(()) => {
                    succeeded = true;
                    break 'retry;
                }
                Err(msg) => {
                    last_error = msg;
                    // Transient failure — will retry if attempts remain (loop continues).
                }
            }
        }

        if !succeeded && !last_error.is_empty() {
            tracing::error!(
                bin = %bin,
                job_id = %tick.job_id,
                attempts = actual_attempts,
                error = %last_error,
                "Spawn failed after all retries"
            );
            if max_retries > 0 {
                publish_dead_letter(&publisher, &tick, "spawn", actual_attempts, last_error).await;
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

    // Validate retry config.
    if let Some(retry) = &config.retry {
        if retry.retry_backoff_sec == 0 {
            return Err(CronError::InvalidJobConfig {
                reason: "retry_backoff_sec must be >= 1".into(),
            });
        }
        if retry.max_retries > 10 {
            return Err(CronError::InvalidJobConfig {
                reason: format!("max_retries must be <= 10, got {}", retry.max_retries),
            });
        }
        if retry.max_retry_duration_sec == Some(0) {
            return Err(CronError::InvalidJobConfig {
                reason: "max_retry_duration_sec must be >= 1 when set".into(),
            });
        }
        if retry.max_backoff_sec == Some(0) {
            return Err(CronError::InvalidJobConfig {
                reason: "max_backoff_sec must be >= 1 when set".into(),
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
    use crate::config::{Action, RetryConfig, Schedule};

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
                retry: None,
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
            let t1: crate::config::TickPayload =
                serde_json::from_slice(&ticks[0].payload).unwrap();
            let t2: crate::config::TickPayload =
                serde_json::from_slice(&ticks[1].payload).unwrap();
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
                retry: None,
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
            retry: None,
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
            retry: None,
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
            retry: None,
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
            retry: None,
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
            retry: None,
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
            retry: None,
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
            retry: None,
        };
        assert!(build_job_state(config).is_ok());
    }

    #[test]
    fn retry_backoff_zero_is_rejected() {
        let config = JobConfig {
            id: "bad-retry".to_string(),
            schedule: Schedule::Interval { interval_sec: 60 },
            action: Action::Publish { subject: "cron.bad-retry".to_string() },
            enabled: true,
            payload: None,
            retry: Some(RetryConfig { max_retries: 3, retry_backoff_sec: 0, max_backoff_sec: None, max_retry_duration_sec: None }),
        };
        let err = build_job_state(config).err().unwrap();
        assert!(err.to_string().contains("retry_backoff_sec"));
    }

    #[test]
    fn retry_config_valid_is_accepted() {
        let config = JobConfig {
            id: "good-retry".to_string(),
            schedule: Schedule::Interval { interval_sec: 60 },
            action: Action::Publish { subject: "cron.good-retry".to_string() },
            enabled: true,
            payload: None,
            retry: Some(RetryConfig { max_retries: 3, retry_backoff_sec: 5, max_backoff_sec: None, max_retry_duration_sec: None }),
        };
        assert!(build_job_state(config).is_ok());
    }

    #[test]
    fn max_retries_over_10_is_rejected() {
        let config = JobConfig {
            id: "too-many-retries".to_string(),
            schedule: Schedule::Interval { interval_sec: 60 },
            action: Action::Publish { subject: "cron.too-many".to_string() },
            enabled: true,
            payload: None,
            retry: Some(RetryConfig { max_retries: 11, retry_backoff_sec: 1, max_backoff_sec: None, max_retry_duration_sec: None }),
        };
        let err = build_job_state(config).err().unwrap();
        assert!(err.to_string().contains("max_retries"));
    }

    #[test]
    fn max_retries_at_10_is_accepted() {
        let config = JobConfig {
            id: "max-retries-10".to_string(),
            schedule: Schedule::Interval { interval_sec: 60 },
            action: Action::Publish { subject: "cron.max10".to_string() },
            enabled: true,
            payload: None,
            retry: Some(RetryConfig { max_retries: 10, retry_backoff_sec: 1, max_backoff_sec: None, max_retry_duration_sec: None }),
        };
        assert!(build_job_state(config).is_ok());
    }

    #[test]
    fn max_retry_duration_zero_is_rejected() {
        let config = JobConfig {
            id: "bad-duration".to_string(),
            schedule: Schedule::Interval { interval_sec: 60 },
            action: Action::Publish { subject: "cron.bad-duration".to_string() },
            enabled: true,
            payload: None,
            retry: Some(RetryConfig { max_retries: 3, retry_backoff_sec: 1, max_backoff_sec: None, max_retry_duration_sec: Some(0) }),
        };
        let err = build_job_state(config).err().unwrap();
        assert!(err.to_string().contains("max_retry_duration_sec"));
    }

    #[test]
    fn max_retry_duration_valid_is_accepted() {
        let config = JobConfig {
            id: "good-duration".to_string(),
            schedule: Schedule::Interval { interval_sec: 60 },
            action: Action::Publish { subject: "cron.good-duration".to_string() },
            enabled: true,
            payload: None,
            retry: Some(RetryConfig { max_retries: 3, retry_backoff_sec: 1, max_backoff_sec: None, max_retry_duration_sec: Some(30) }),
        };
        assert!(build_job_state(config).is_ok());
    }

    // ── retry_backoff() unit tests ────────────────────────────────────────────

    #[test]
    fn retry_backoff_attempt_1_returns_base() {
        assert_eq!(retry_backoff(2, 1), Duration::from_secs(2));
    }

    #[test]
    fn retry_backoff_attempt_2_doubles() {
        assert_eq!(retry_backoff(2, 2), Duration::from_secs(4));
    }

    #[test]
    fn retry_backoff_attempt_3_quadruples() {
        assert_eq!(retry_backoff(2, 3), Duration::from_secs(8));
    }

    #[test]
    fn retry_backoff_capped_at_32x() {
        // attempt=6 would be 2^5=32x, attempt=7 would be 2^6=64x but cap is 32x
        assert_eq!(retry_backoff(1, 6), Duration::from_secs(32));
        assert_eq!(retry_backoff(1, 7), Duration::from_secs(32));
        assert_eq!(retry_backoff(1, 100), Duration::from_secs(32));
    }
    #[test]
    fn max_backoff_sec_zero_is_rejected() {
        let config = JobConfig {
            id: "bad-maxbo".to_string(),
            schedule: Schedule::Interval { interval_sec: 60 },
            action: Action::Publish { subject: "cron.bad-maxbo".to_string() },
            enabled: true,
            payload: None,
            retry: Some(RetryConfig {
                max_retries: 3,
                retry_backoff_sec: 1,
                max_backoff_sec: Some(0),
                max_retry_duration_sec: None,
            }),
        };
        let err = build_job_state(config).err().unwrap();
        assert!(err.to_string().contains("max_backoff_sec"));
    }

    #[test]
    fn max_backoff_sec_below_base_is_accepted() {
        // max_backoff_sec < retry_backoff_sec is valid: the cap applies on every attempt,
        // effectively making all delays equal to max_backoff_sec.
        let config = JobConfig {
            id: "maxbo-below-base".to_string(),
            schedule: Schedule::Interval { interval_sec: 60 },
            action: Action::Publish { subject: "cron.maxbo-below-base".to_string() },
            enabled: true,
            payload: None,
            retry: Some(RetryConfig {
                max_retries: 3,
                retry_backoff_sec: 30,
                max_backoff_sec: Some(2),
                max_retry_duration_sec: None,
            }),
        };
        assert!(build_job_state(config).is_ok());
    }

    #[test]
    fn max_backoff_sec_valid_is_accepted() {
        let config = JobConfig {
            id: "good-maxbo".to_string(),
            schedule: Schedule::Interval { interval_sec: 60 },
            action: Action::Publish { subject: "cron.good-maxbo".to_string() },
            enabled: true,
            payload: None,
            retry: Some(RetryConfig {
                max_retries: 3,
                retry_backoff_sec: 5,
                max_backoff_sec: Some(30),
                max_retry_duration_sec: None,
            }),
        };
        assert!(build_job_state(config).is_ok());
    }

    #[test]
    fn retry_backoff_with_jitter_respects_max_backoff() {
        // With base=60 attempt=1 → base delay 60s; cap at 2s → delay must be <= 2s
        let max = Duration::from_secs(2);
        for _ in 0..20 {
            let d = retry_backoff_with_jitter(60, 1, Some(max));
            assert!(d <= max, "delay {d:?} exceeded max_backoff {max:?}");
        }
    }

}
