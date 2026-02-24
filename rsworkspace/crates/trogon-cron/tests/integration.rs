//! Integration tests — require a running NATS server.
//!
//! Run with:
//!   NATS_TEST_URL=nats://localhost:4222 cargo test -p trogon-cron --test integration -- --include-ignored --test-threads=1
//!
//! `--test-threads=1` is required: several tests start a `Scheduler` and share
//! the same NATS leader-election bucket.  Running them in parallel causes
//! `reset_leader_lock` calls from one test to steal leadership from another.
//!
//! These tests are marked `#[ignore]` so they don't run in CI without NATS.

use std::time::Duration;

use async_nats::jetstream::{self, consumer};
use futures::StreamExt;
use trogon_cron::{Action, CronClient, JobConfig, Schedule, Scheduler};
use std::path::PathBuf;

// Type alias so callers don't need to spell out the push consumer stream type.
type CronStream = consumer::push::Messages;

fn test_url() -> String {
    std::env::var("NATS_TEST_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string())
}

async fn connect() -> async_nats::Client {
    async_nats::connect(test_url())
        .await
        .expect("Failed to connect to NATS — is NATS_TEST_URL set and NATS running?")
}

/// Connect and return both the NATS client and a JetStream context.
/// Also ensures the CRON_TICKS stream exists so consumers can be
/// created before the scheduler starts.
async fn connect_js() -> (async_nats::Client, jetstream::Context) {
    let nats = connect().await;
    let js = jetstream::new(nats.clone());
    trogon_cron::kv::get_or_create_ticks_stream(&js)
        .await
        .expect("Failed to create CRON_TICKS stream");
    (nats, js)
}

/// Subscribe to `subject` via a JetStream push consumer on CRON_TICKS.
///
/// `push::Config::messages()` creates the Core NATS subscription **eagerly** inside
/// the async fn before returning — so by the time `cron_messages` resolves the inbox
/// is already registered with the server.  Any ticks the scheduler publishes after
/// this point are delivered directly to the subscriber with no race window.
///
/// This contrasts with pull consumers (where the `MSG.NEXT` fetch is deferred until
/// the first `poll_next` call) and ordered push consumers (which also lazily set up
/// the subscription), both of which can miss a tick that fires before polling begins.
async fn cron_messages(
    nats: &async_nats::Client,
    js: &jetstream::Context,
    subject: String,
) -> CronStream {
    let inbox = nats.new_inbox();
    js.get_stream(trogon_cron::kv::TICKS_STREAM)
        .await
        .expect("CRON_TICKS stream not found")
        .create_consumer(consumer::push::Config {
            deliver_subject: inbox,
            filter_subject: subject,
            deliver_policy: consumer::DeliverPolicy::All,
            ack_policy: consumer::AckPolicy::None,
            ..Default::default()
        })
        .await
        .expect("Failed to create consumer")
        .messages()
        .await
        .expect("Failed to start message stream")
}

fn unique_id(prefix: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    format!("{prefix}-{ts}")
}

/// Delete the leader lock so the next scheduler can acquire leadership immediately.
/// Without this, an aborted scheduler leaves a stale lock for up to 10 s (the KV TTL),
/// which would cause the following test's scheduler to fail to become leader within the
/// 5-second test timeout.
async fn reset_leader_lock(js: &jetstream::Context) {
    if let Ok(kv) = js.get_key_value(trogon_cron::kv::LEADER_BUCKET).await {
        let _ = kv.purge(trogon_cron::kv::LEADER_KEY).await;
    }
}

// ── CronClient CRUD ──────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_register_and_list_job() {
    let nats = connect().await;
    let client = CronClient::new(nats).await.unwrap();
    let id = unique_id("test-list");

    let job = JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 60 },
        action: Action::Publish { subject: "cron.tick".to_string() },
        enabled: true,
        payload: None,
    };

    client.register_job(&job).await.unwrap();

    let jobs = client.list_jobs().await.unwrap();
    assert!(jobs.iter().any(|j| j.id == id), "registered job should appear in list");

    // Cleanup
    client.remove_job(&id).await.unwrap();
    let jobs = client.list_jobs().await.unwrap();
    assert!(!jobs.iter().any(|j| j.id == id), "removed job should not appear in list");
}

#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_get_job() {
    let nats = connect().await;
    let client = CronClient::new(nats).await.unwrap();
    let id = unique_id("test-get");

    assert!(client.get_job(&id).await.unwrap().is_none(), "should not exist yet");

    let job = JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 10 },
        action: Action::Publish { subject: "cron.get".to_string() },
        enabled: true,
        payload: Some(serde_json::json!({ "key": "value" })),
    };
    client.register_job(&job).await.unwrap();

    let fetched = client.get_job(&id).await.unwrap().expect("job should exist");
    assert_eq!(fetched.id, id);
    assert!(fetched.payload.is_some());

    client.remove_job(&id).await.unwrap();
}

#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_enable_disable_job() {
    let nats = connect().await;
    let client = CronClient::new(nats).await.unwrap();
    let id = unique_id("test-toggle");

    let job = JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 60 },
        action: Action::Publish { subject: "cron.toggle".to_string() },
        enabled: true,
        payload: None,
    };
    client.register_job(&job).await.unwrap();

    client.set_enabled(&id, false).await.unwrap();
    assert!(!client.get_job(&id).await.unwrap().unwrap().enabled);

    client.set_enabled(&id, true).await.unwrap();
    assert!(client.get_job(&id).await.unwrap().unwrap().enabled);

    client.remove_job(&id).await.unwrap();
}

// ── Scheduler fires jobs ─────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_interval_job_fires() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-fire");
    // Subject must start with `cron.` to be captured by the CRON_TICKS stream.
    let subject = format!("cron.fire.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    let job = JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: Some(serde_json::json!({ "test": true })),
    };
    client.register_job(&job).await.unwrap();

    // Clear any stale leader lock from a previous test so this scheduler can become
    // leader immediately rather than waiting up to 10 s for the TTL to expire.
    reset_leader_lock(&js).await;

    // Run scheduler in the background
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Wait up to 5 seconds for the first tick
    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("Timed out waiting for tick — scheduler may not have fired")
        .expect("Subscription closed unexpectedly")
        .expect("JetStream message error");

    // Verify payload structure
    let payload: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(payload["job_id"], id.as_str());
    assert!(payload["fired_at"].is_string());
    assert!(payload["execution_id"].is_string());
    assert_eq!(payload["payload"]["test"], true);

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_hot_reload_job_config() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-reload");
    let subject_v1 = format!("cron.reload-v1.{id}");
    let subject_v2 = format!("cron.reload-v2.{id}");

    let mut sub_v2 = cron_messages(&nats, &js, subject_v2.clone()).await;

    // Register v1 job
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject_v1.clone() },
        enabled: true,
        payload: None,
    }).await.unwrap();

    // Clear any stale leader lock so this scheduler acquires leadership immediately.
    reset_leader_lock(&js).await;

    // Start scheduler
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Give the scheduler time to start and pick up v1
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Hot-update job to publish to v2
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject_v2.clone() },
        enabled: true,
        payload: None,
    }).await.unwrap();

    // Within 5 seconds, we should receive a tick on v2
    let msg = tokio::time::timeout(Duration::from_secs(5), sub_v2.next())
        .await
        .expect("Timed out — scheduler did not pick up updated config")
        .expect("Subscription closed")
        .expect("JetStream message error");

    let payload: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(payload["job_id"], id.as_str());

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_disabled_job_does_not_fire() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-disabled");
    let subject = format!("cron.disabled.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: false,
        payload: None,
    }).await.unwrap();

    // Clear any stale leader lock so this scheduler acquires leadership immediately.
    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Wait 3 seconds — should receive NO ticks
    let result: Result<_, _> = tokio::time::timeout(Duration::from_secs(3), sub.next()).await;
    assert!(result.is_err(), "Disabled job should not fire");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Action::Spawn ─────────────────────────────────────────────────────────────

/// Verify that a `Spawn` job actually executes the configured binary.
///
/// Uses `/usr/bin/touch` to create a temp file as an observable side-effect,
/// since spawn jobs don't publish NATS messages.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_job_fires() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-spawn");
    let tmp: PathBuf = std::env::temp_dir().join(format!("trogon-cron-spawn-{id}"));

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/usr/bin/touch".to_string(),
            args: vec![tmp.to_str().unwrap().to_string()],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Poll until the file appears or 5-second deadline.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if tmp.exists() { break; }
        assert!(tokio::time::Instant::now() < deadline, "Timed out — spawn job never fired");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&tmp);
}

/// Verify that the tick context is injected as environment variables into the
/// spawned process (`CRON_JOB_ID`, `CRON_FIRED_AT`, `CRON_EXECUTION_ID`, `CRON_PAYLOAD`).
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_job_env_vars() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-spawn-env");
    let tmp: PathBuf = std::env::temp_dir().join(format!("trogon-cron-env-{id}"));

    // The shell script writes CRON_JOB_ID to the temp file so we can assert its value.
    let script = format!("printf '%s' \"$CRON_JOB_ID\" > '{}'", tmp.to_str().unwrap());

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: Some(serde_json::json!({ "key": "value" })),
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Poll until the file appears or 5-second deadline.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if tmp.exists() { break; }
        assert!(tokio::time::Instant::now() < deadline, "Timed out — spawn job never fired");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // CRON_JOB_ID must equal the job id.
    let content = std::fs::read_to_string(&tmp).unwrap();
    assert_eq!(content.trim(), id.as_str(), "CRON_JOB_ID env var not set correctly");

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&tmp);
}

/// Verify that `concurrent: false` skips a tick when the previous invocation
/// is still running.
///
/// Strategy: spawn a job that sleeps 10 s (longer than the test window) and
/// writes its PID to a temp file on start.  After the first invocation is
/// confirmed running, wait for at least two more scheduler ticks and verify
/// only one PID file was ever created (i.e., the second tick was skipped).
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_concurrent_false_skips_while_running() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-concurrent");
    let pid_file: PathBuf = std::env::temp_dir().join(format!("trogon-cron-concurrent-{id}"));
    let counter_file: PathBuf =
        std::env::temp_dir().join(format!("trogon-cron-concurrent-count-{id}"));

    // Each invocation appends a line to counter_file, then sleeps 10 s.
    // With concurrent: false and interval 1 s, only the first invocation should run.
    let script = format!(
        "echo x >> '{}'; sleep 10",
        counter_file.to_str().unwrap()
    );
    let _ = std::fs::remove_file(&counter_file);

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script],
            concurrent: false,
            timeout_sec: Some(15),
        },
        enabled: true,
        payload: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Wait for the first invocation to start (counter_file has at least one line).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if counter_file.exists() { break; }
        assert!(tokio::time::Instant::now() < deadline, "First invocation never started");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Wait 3 more seconds (≥3 ticks at interval=1s).  Since the first process is
    // still sleeping, every subsequent tick must be skipped.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let line_count = std::fs::read_to_string(&counter_file)
        .unwrap_or_default()
        .lines()
        .count();
    assert_eq!(line_count, 1, "concurrent: false should prevent re-entry; got {line_count} invocations");

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&counter_file);
    let _ = std::fs::remove_file(&pid_file);
}

// ── Schedule::Cron ────────────────────────────────────────────────────────────

/// Verify that a job using a cron expression fires correctly.
/// Uses `* * * * * *` (every second) so the test doesn't need to wait long.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_cron_expression_job_fires() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-cron-expr");
    let subject = format!("cron.expr.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Cron { expr: "* * * * * *".to_string() }, // every second
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("Timed out waiting for cron expression job to fire")
        .expect("Subscription closed")
        .expect("JetStream message error");

    let payload: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(payload["job_id"], id.as_str());
    assert!(payload["fired_at"].is_string());

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Multiple jobs ─────────────────────────────────────────────────────────────

/// Verify that multiple jobs registered concurrently all fire independently.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_multiple_jobs_fire_independently() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id_a = unique_id("test-multi-a");
    let id_b = unique_id("test-multi-b");
    let subject_a = format!("cron.multi.a.{id_a}");
    let subject_b = format!("cron.multi.b.{id_b}");

    let mut sub_a = cron_messages(&nats, &js, subject_a.clone()).await;
    let mut sub_b = cron_messages(&nats, &js, subject_b.clone()).await;

    client.register_job(&JobConfig {
        id: id_a.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject_a.clone() },
        enabled: true,
        payload: None,
    }).await.unwrap();

    client.register_job(&JobConfig {
        id: id_b.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject_b.clone() },
        enabled: true,
        payload: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Both jobs must fire within 5 seconds.
    let (msg_a, msg_b) = tokio::time::timeout(
        Duration::from_secs(5),
        async { (sub_a.next().await, sub_b.next().await) },
    )
    .await
    .expect("Timed out — not all jobs fired");

    let pa: serde_json::Value = serde_json::from_slice(&msg_a.unwrap().unwrap().payload).unwrap();
    let pb: serde_json::Value = serde_json::from_slice(&msg_b.unwrap().unwrap().payload).unwrap();
    assert_eq!(pa["job_id"], id_a.as_str());
    assert_eq!(pb["job_id"], id_b.as_str());

    handle.abort();
    client.remove_job(&id_a).await.unwrap();
    client.remove_job(&id_b).await.unwrap();
}

// ── Re-enable ─────────────────────────────────────────────────────────────────

/// Verify that a job disabled at registration time fires after being re-enabled.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_reenable_fires_after_disabled() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-reenable");
    let subject = format!("cron.reenable.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    // Register as disabled — should never fire while disabled.
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: false,
        payload: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Confirm no tick arrives while disabled (2 s window).
    let silent = tokio::time::timeout(Duration::from_secs(2), sub.next()).await;
    assert!(silent.is_err(), "Disabled job should not fire");

    // Re-enable — scheduler hot-reloads and starts firing.
    client.set_enabled(&id, true).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("Timed out — job did not fire after re-enable")
        .expect("Subscription closed")
        .expect("JetStream message error");

    let payload: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(payload["job_id"], id.as_str());

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Remove stops ticks ────────────────────────────────────────────────────────

/// Verify that removing a job causes the scheduler to stop firing it.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_remove_job_stops_ticks() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-remove-stops");
    let subject = format!("cron.remove.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Wait for at least one tick to confirm the job is running.
    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("Timed out waiting for first tick")
        .expect("Subscription closed")
        .expect("JetStream message error");

    // Remove the job — scheduler hot-reloads within ~500 ms.
    client.remove_job(&id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;

    // No further ticks should arrive after removal.
    let after_removal = tokio::time::timeout(Duration::from_secs(2), sub.next()).await;
    assert!(after_removal.is_err(), "Job should not fire after removal");

    handle.abort();
}

// ── Spawn timeout ─────────────────────────────────────────────────────────────

/// Verify that `timeout_sec` actually kills a long-running process.
///
/// The script writes a "started" marker immediately, then sleeps 60 s,
/// then writes a "done" marker.  With `timeout_sec: 2` the process should be
/// killed before the sleep completes — so "started" must exist but "done" must not.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_timeout_kills_process() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-spawn-timeout");
    let started: PathBuf = std::env::temp_dir().join(format!("trogon-cron-started-{id}"));
    let done: PathBuf    = std::env::temp_dir().join(format!("trogon-cron-done-{id}"));
    let _ = std::fs::remove_file(&started);
    let _ = std::fs::remove_file(&done);

    let script = format!(
        "touch '{}'; sleep 60; touch '{}'",
        started.to_str().unwrap(),
        done.to_str().unwrap(),
    );

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script],
            concurrent: false,
            timeout_sec: Some(2), // kill after 2 s, well before sleep 60 ends
        },
        enabled: true,
        payload: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Wait for the process to start.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if started.exists() { break; }
        assert!(tokio::time::Instant::now() < deadline, "Process never started");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Wait long enough for the timeout + SIGTERM grace period to fire (2s timeout + 1s margin).
    tokio::time::sleep(Duration::from_secs(4)).await;

    // The process was killed — "done" must not exist.
    assert!(!done.exists(), "Process completed despite timeout — kill did not work");

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&started);
    let _ = std::fs::remove_file(&done);
}

// ── Leader election failover ──────────────────────────────────────────────────

/// Verify that when the active leader crashes (lock NOT released gracefully),
/// a second scheduler acquires leadership after the TTL expires (≤ 10 s) and
/// resumes firing ticks automatically.
///
/// This test intentionally does NOT call `reset_leader_lock` between the two
/// schedulers — that is the whole point: the failover must work without manual
/// intervention.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_leader_failover() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-failover");
    let subject = format!("cron.failover.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    // Start scheduler A — becomes the initial leader.
    let nats_a = nats.clone();
    let handle_a = tokio::spawn(async move {
        Scheduler::new(nats_a).run().await.ok();
    });

    // Confirm A is leader and firing ticks.
    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("Scheduler A never fired — not leader")
        .unwrap()
        .unwrap();

    // Kill A — simulates a crash. Lock is NOT released; it must expire naturally.
    handle_a.abort();

    // Start scheduler B immediately. It must wait for the TTL to expire (≤ 10 s).
    let nats_b = nats.clone();
    let handle_b = tokio::spawn(async move {
        Scheduler::new(nats_b).run().await.ok();
    });

    // Allow up to 15 s: worst-case TTL (10 s) + scheduler startup + one tick interval.
    tokio::time::timeout(Duration::from_secs(15), sub.next())
        .await
        .expect("Scheduler B did not take over within 15 s after leader crash")
        .unwrap()
        .unwrap();

    handle_b.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Graceful shutdown releases lock ──────────────────────────────────────────

/// Verify that SIGTERM causes the scheduler to release the leader lock
/// immediately, so the next scheduler takes over in < 3 s instead of waiting
/// for the 10-second TTL.
///
/// Runs the real binary as a subprocess so SIGTERM can be sent via kill(2).
#[cfg(unix)]
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_graceful_shutdown_releases_lock() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-shutdown");
    let subject = format!("cron.shutdown.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    // Spawn the real binary so SIGTERM targets only that OS process.
    let bin = env!("CARGO_BIN_EXE_trogon-cron");
    let mut child = tokio::process::Command::new(bin)
        .args(["serve"])
        .env("NATS_URL", test_url())
        .env("RUST_LOG", "error")
        .spawn()
        .expect("Failed to spawn trogon-cron binary");

    // Wait for the binary to become leader and fire at least one tick.
    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("Binary scheduler never fired — not leader")
        .unwrap()
        .unwrap();

    // Send SIGTERM — scheduler releases the lock and exits cleanly.
    let pid = child.id().expect("No PID");
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid as i32),
        nix::sys::signal::Signal::SIGTERM,
    ).expect("kill(SIGTERM) failed");

    tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("Binary did not exit within 10 s after SIGTERM")
        .unwrap();

    // Start a second scheduler. Because the lock was released, it must acquire
    // leadership and fire within 3 s — NOT waiting up to 10 s for the TTL.
    let nats2 = nats.clone();
    let handle2 = tokio::spawn(async move {
        Scheduler::new(nats2).run().await.ok();
    });

    tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await
        .expect("Second scheduler did not acquire lock quickly — lock may not have been released on SIGTERM")
        .unwrap()
        .unwrap();

    handle2.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Payload forwarding ────────────────────────────────────────────────────────

/// Verify that the `payload` field from JobConfig is included verbatim in the
/// TickPayload published to NATS, and that all standard fields are present.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_tick_payload_forwarded() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-payload");
    let subject = format!("cron.payload.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    let custom_payload = serde_json::json!({
        "db": "main",
        "region": "us-east-1",
        "retries": 3
    });

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: Some(custom_payload.clone()),
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("Timed out waiting for tick")
        .unwrap()
        .unwrap();

    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(tick.job_id, id, "job_id mismatch");
    assert!(!tick.execution_id.is_empty(), "execution_id should be a non-empty UUID");
    assert!(tick.payload.is_some(), "payload field should be present in tick");
    assert_eq!(tick.payload.unwrap(), custom_payload, "payload content must match exactly");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── concurrent: true ─────────────────────────────────────────────────────────

/// Verify that `concurrent: true` allows multiple invocations to overlap.
///
/// Each invocation sleeps 5 s after appending to a counter file.
/// With `interval_sec: 1` and `concurrent: true`, after 4 s there must be
/// at least 3 overlapping invocations running simultaneously.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_concurrent_true_allows_overlap() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-concurrent-true");
    let counter: PathBuf = std::env::temp_dir().join(format!("trogon-cron-ctrue-{id}"));
    let _ = std::fs::remove_file(&counter);

    // Each invocation appends a line then sleeps 5 s.
    // With concurrent: true and interval 1s, multiple must start before any finish.
    let script = format!("echo x >> '{}'; sleep 5", counter.to_str().unwrap());

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script],
            concurrent: true,
            timeout_sec: Some(10),
        },
        enabled: true,
        payload: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Wait 4 s — enough for ≥ 3 invocations at 1 s interval.
    tokio::time::sleep(Duration::from_secs(4)).await;

    let line_count = std::fs::read_to_string(&counter)
        .unwrap_or_default()
        .lines()
        .count();

    assert!(
        line_count >= 3,
        "concurrent: true should allow overlapping invocations; got {line_count}"
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&counter);
}

// ── Publish subject validation (network) ─────────────────────────────────────

/// Verify that `register_job` rejects a publish job whose subject does not
/// start with `cron.` — over a real NATS connection, not just in unit tests.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_register_job_rejects_non_cron_subject() {
    let nats = connect().await;
    let client = CronClient::new(nats).await.unwrap();

    let result = client.register_job(&JobConfig {
        id: unique_id("bad-subject"),
        schedule: Schedule::Interval { interval_sec: 60 },
        action: Action::Publish { subject: "events.backup".to_string() },
        enabled: true,
        payload: None,
    }).await;

    assert!(result.is_err(), "register_job should reject non-cron. subject");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("cron."),
        "error message should mention 'cron.' prefix: {err}"
    );
}

// ── CLI binary ────────────────────────────────────────────────────────────────

/// Verify the full CLI lifecycle: add → list → get → disable → enable → remove.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_cli_job_lifecycle() {
    let bin = env!("CARGO_BIN_EXE_trogon-cron");
    let id = unique_id("test-cli");
    let nats_url = test_url();
    let subject = format!("cron.cli.{id}");

    // Write job JSON to a temp file so we can pass it as a file argument.
    let tmp = std::env::temp_dir().join(format!("trogon-cli-{id}.json"));
    let job_json = serde_json::json!({
        "id": id,
        "schedule": { "type": "interval", "interval_sec": 60 },
        "action": { "type": "publish", "subject": subject },
        "enabled": true
    }).to_string();
    std::fs::write(&tmp, &job_json).unwrap();

    // job add <file>
    let out = tokio::process::Command::new(bin)
        .args(["--nats-url", &nats_url, "job", "add", tmp.to_str().unwrap()])
        .env("RUST_LOG", "error")
        .output().await.unwrap();
    assert!(out.status.success(), "job add failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains(&id));

    // job list — job must appear
    let out = tokio::process::Command::new(bin)
        .args(["--nats-url", &nats_url, "job", "list"])
        .env("RUST_LOG", "error")
        .output().await.unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains(&id));

    // job get — returns valid JSON with correct id and enabled: true
    let out = tokio::process::Command::new(bin)
        .args(["--nats-url", &nats_url, "job", "get", &id])
        .env("RUST_LOG", "error")
        .output().await.unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["id"], id.as_str());
    assert_eq!(json["enabled"], true);

    // job disable
    let out = tokio::process::Command::new(bin)
        .args(["--nats-url", &nats_url, "job", "disable", &id])
        .env("RUST_LOG", "error")
        .output().await.unwrap();
    assert!(out.status.success());
    let out = tokio::process::Command::new(bin)
        .args(["--nats-url", &nats_url, "job", "get", &id])
        .env("RUST_LOG", "error")
        .output().await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["enabled"], false, "job should be disabled");

    // job enable
    let out = tokio::process::Command::new(bin)
        .args(["--nats-url", &nats_url, "job", "enable", &id])
        .env("RUST_LOG", "error")
        .output().await.unwrap();
    assert!(out.status.success());
    let out = tokio::process::Command::new(bin)
        .args(["--nats-url", &nats_url, "job", "get", &id])
        .env("RUST_LOG", "error")
        .output().await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["enabled"], true, "job should be re-enabled");

    // job remove
    let out = tokio::process::Command::new(bin)
        .args(["--nats-url", &nats_url, "job", "remove", &id])
        .env("RUST_LOG", "error")
        .output().await.unwrap();
    assert!(out.status.success());

    // job get after removal must exit with error
    let out = tokio::process::Command::new(bin)
        .args(["--nats-url", &nats_url, "job", "get", &id])
        .env("RUST_LOG", "error")
        .output().await.unwrap();
    assert!(!out.status.success(), "job get should fail after removal");

    let _ = std::fs::remove_file(&tmp);
}

// ── Invalid KV entry ignored ──────────────────────────────────────────────────

/// Verify that a malformed JSON entry written directly to the KV bucket
/// (bypassing register_job) is silently ignored by the scheduler, which
/// continues to fire valid jobs normally.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_invalid_job_config_in_kv_is_ignored() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-invalid-kv");
    let subject = format!("cron.invalid-kv.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    // Write malformed JSON directly to KV, bypassing register_job validation.
    let kv = js.get_key_value(trogon_cron::kv::CONFIG_BUCKET).await.unwrap();
    let bad_key = format!("{}bad-{id}", trogon_cron::kv::JOBS_KEY_PREFIX);
    kv.put(bad_key.clone(), "{ not valid json !!!".into()).await.unwrap();

    // Register a valid job alongside the bad entry.
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // The valid job must still fire — the bad entry must not crash the scheduler.
    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("Scheduler crashed or ignored valid job due to invalid KV entry")
        .unwrap()
        .unwrap();

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = kv.delete(bad_key).await;
}

// ── Cron expression with step value ──────────────────────────────────────────

/// Verify that a cron expression with a step value (`*/2 * * * * *`) is
/// parsed and fired correctly — tests a more complex expression than `* * * * * *`.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_cron_step_expression_fires() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-cron-step");
    let subject = format!("cron.step.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    // */2 * * * * * fires every even second — tests step value parsing.
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Cron { expr: "*/2 * * * * *".to_string() },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("Cron expression */2 * * * * * never fired within 5 s")
        .unwrap()
        .unwrap();

    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(tick.job_id, id);
    assert!(!tick.execution_id.is_empty());

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── payload: None ─────────────────────────────────────────────────────────────

/// Verify that when a job has no payload configured, the TickPayload published
/// to NATS has `payload: null` (field absent in JSON) and all other fields
/// are present and correct.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_tick_without_payload() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-no-payload");
    let subject = format!("cron.nopayload.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("Timed out waiting for tick")
        .unwrap()
        .unwrap();

    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(tick.job_id, id);
    assert!(!tick.execution_id.is_empty(), "execution_id must be present");
    assert!(tick.fired_at <= chrono::Utc::now(), "fired_at must be a past timestamp");
    assert!(tick.payload.is_none(), "payload should be None when not configured");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Interval fires multiple times ─────────────────────────────────────────────

/// Verify that an interval job fires repeatedly, not just once.
///
/// Waits for three consecutive ticks and asserts each has a unique
/// `execution_id`, proving the scheduler reschedules after each fire.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_interval_job_fires_multiple_times() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-multi-tick");
    let subject = format!("cron.multitick.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Collect 3 consecutive ticks — each must arrive within 5 s of the previous.
    let mut execution_ids = Vec::new();
    for i in 0..3 {
        let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
            .await
            .unwrap_or_else(|_| panic!("Timed out waiting for tick #{}", i + 1))
            .unwrap()
            .unwrap();
        let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
        assert_eq!(tick.job_id, id, "job_id mismatch on tick #{}", i + 1);
        execution_ids.push(tick.execution_id);
    }

    // All three execution_ids must be distinct — each tick is a fresh invocation.
    let unique: std::collections::HashSet<_> = execution_ids.iter().collect();
    assert_eq!(unique.len(), 3, "all 3 ticks should have distinct execution_ids");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Hot-add: new job registered while scheduler is already running ─────────────

/// Verify that a job registered AFTER the scheduler has started is picked up
/// automatically via the KV config watcher and begins firing.
///
/// This is distinct from `test_hot_reload_job_config`, which updates a job
/// that was already present at scheduler startup.  Here the scheduler starts
/// with NO entry for this job id and must react to a brand-new KV Put.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_hot_add_job_while_scheduler_running() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-hot-add");
    let subject = format!("cron.hotadd.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    reset_leader_lock(&js).await;

    // Start the scheduler before any job for this id exists.
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Wait for the scheduler to stabilise (initial load + leader election).
    tokio::time::sleep(Duration::from_millis(800)).await;

    // Now register the new job — scheduler must discover it via the watcher.
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
    }).await.unwrap();

    // The new job must fire within 5 s of registration.
    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("Timed out — scheduler did not pick up newly registered job")
        .unwrap()
        .unwrap();

    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(tick.job_id, id);

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── concurrent: false re-fires after process completes ───────────────────────

/// Verify that `concurrent: false` allows re-firing once the previous
/// invocation finishes.
///
/// Each invocation appends a line and exits immediately (<100 ms), so
/// after 3 s at interval=1 s the counter must show ≥ 2 invocations.
/// This proves the `is_running` flag is cleared after process exit.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_concurrent_false_re_fires_after_completion() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-refire");
    let counter: PathBuf = std::env::temp_dir().join(format!("trogon-cron-refire-{id}"));
    let _ = std::fs::remove_file(&counter);

    // Each invocation appends one line and exits immediately.
    let script = format!("echo x >> '{}'", counter.to_str().unwrap());

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // After 3 s at interval=1 s, at least 2 quick invocations must have run.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let line_count = std::fs::read_to_string(&counter)
        .unwrap_or_default()
        .lines()
        .count();

    assert!(
        line_count >= 2,
        "concurrent: false must allow re-firing after completion; got {line_count} invocations"
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&counter);
}

// ── Spawn with no timeout completes normally ──────────────────────────────────

/// Verify that a `Spawn` job with `timeout_sec: None` is never killed
/// prematurely — the process runs to full completion.
///
/// The process sleeps 1 s then writes a sentinel file.  Any finite timeout
/// shorter than 1 s would kill it before the write; `None` must allow it
/// to finish.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_no_timeout_completes_normally() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-no-timeout");
    let output: PathBuf = std::env::temp_dir().join(format!("trogon-cron-notimeout-{id}"));
    let _ = std::fs::remove_file(&output);

    // Sleep 1 s then write the sentinel file.
    let script = format!("sleep 1 && touch '{}'", output.to_str().unwrap());

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script],
            concurrent: false,
            timeout_sec: None, // no timeout — process must run to completion
        },
        enabled: true,
        payload: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Poll until the sentinel file appears (max 8 s: startup + 1 s sleep + margin).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        if output.exists() { break; }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Process never completed — may have been killed despite timeout_sec: None"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&output);
}

// ── Spawn with non-zero exit code doesn't crash the scheduler ─────────────────

/// Verify that a spawned process that exits with a non-zero status code does
/// not crash or stall the scheduler.
///
/// A second, healthy publish job must continue to fire normally alongside the
/// always-failing spawn job.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_failing_exit_code_scheduler_continues() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id_fail = unique_id("test-spawn-fail");
    let id_ok   = unique_id("test-spawn-ok");
    let subject  = format!("cron.spawnfail.{id_ok}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    // Spawn job that always exits with code 1.
    client.register_job(&JobConfig {
        id: id_fail.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "exit 1".to_string()],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: None,
    }).await.unwrap();

    // Healthy publish job — must fire despite the failing spawn.
    client.register_job(&JobConfig {
        id: id_ok.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // The healthy job must fire — proof the failing spawn did not crash the scheduler.
    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("Scheduler stalled after spawned process exited with non-zero code")
        .unwrap()
        .unwrap();

    handle.abort();
    client.remove_job(&id_fail).await.unwrap();
    client.remove_job(&id_ok).await.unwrap();
}

// ── Two schedulers: mutual exclusion ─────────────────────────────────────────

/// Verify that two schedulers running simultaneously do not double-fire jobs.
///
/// With a 1 s interval and a 5 s observation window, a single leader fires
/// ~5 ticks.  If leader election is broken and both fire, the count would
/// approach 10.  We assert the count stays ≤ 7 (generous for timing jitter).
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_two_schedulers_mutual_exclusion() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-mutex");
    let subject = format!("cron.mutex.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    // Start two schedulers simultaneously — only one must become leader.
    let nats_a = nats.clone();
    let handle_a = tokio::spawn(async move {
        Scheduler::new(nats_a).run().await.ok();
    });
    let nats_b = nats.clone();
    let handle_b = tokio::spawn(async move {
        Scheduler::new(nats_b).run().await.ok();
    });

    // Drain all ticks that arrive within 5 s.
    let mut count = 0usize;
    let window = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(window);

    loop {
        tokio::select! {
            biased;
            _ = &mut window => break,
            msg = sub.next() => {
                if msg.is_some() { count += 1; }
            }
        }
    }

    // One scheduler fires ~5 ticks in 5 s.  If both fired we'd see ~10.
    assert!(count >= 3, "too few ticks — scheduler may not have acquired leadership: {count}");
    assert!(count <= 7, "too many ticks — both schedulers may be double-firing: {count}");

    handle_a.abort();
    handle_b.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Interval update while running ─────────────────────────────────────────────

/// Verify that changing a job's interval while the scheduler is running takes
/// effect immediately via hot-reload.
///
/// Strategy:
///   1. Register with interval=10 s — interval jobs fire immediately on load
///      so the first tick arrives right away regardless of the long interval.
///   2. Wait for the first tick to confirm the scheduler is leader.
///   3. Update the job to interval=1 s.
///   4. Assert two more ticks arrive within 5 s — impossible at 10 s cadence.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_update_interval_while_running() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-update-interval");
    let subject = format!("cron.updateinterval.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 10 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Step 1: wait for the first tick (interval jobs fire immediately on load).
    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("First tick never arrived")
        .unwrap()
        .unwrap();

    // Step 2: hot-update to interval=1 s.
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
    }).await.unwrap();

    // Step 3: collect 2 more ticks. At 10 s they would need ~20 s; at 1 s ≤ 5 s.
    for i in 0..2 {
        tokio::time::timeout(Duration::from_secs(5), sub.next())
            .await
            .unwrap_or_else(|_| panic!("Tick #{} did not arrive after interval update", i + 2))
            .unwrap()
            .unwrap();
    }

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Scheduler restart resumes existing jobs ───────────────────────────────────

/// Verify that a freshly started scheduler picks up all jobs that were already
/// stored in NATS KV, even if it has never seen them before.
///
/// This simulates a real production restart: the previous instance crashes,
/// a new pod starts, and jobs must resume without manual intervention.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_scheduler_restart_resumes_jobs() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-restart");
    let subject = format!("cron.restart.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
    }).await.unwrap();

    // Scheduler A — becomes leader and fires the job.
    reset_leader_lock(&js).await;
    let nats_a = nats.clone();
    let handle_a = tokio::spawn(async move {
        Scheduler::new(nats_a).run().await.ok();
    });

    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("Scheduler A never fired")
        .unwrap()
        .unwrap();

    // Simulate crash: abort without releasing the leader lock.
    handle_a.abort();

    // Scheduler B — brand new instance.  reset_leader_lock simulates TTL expiry
    // so B acquires leadership immediately rather than waiting 10 s.
    reset_leader_lock(&js).await;
    let nats_b = nats.clone();
    let handle_b = tokio::spawn(async move {
        Scheduler::new(nats_b).run().await.ok();
    });

    // B must reload the job from KV and resume firing within 5 s.
    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("Scheduler B did not resume the job after restart")
        .unwrap()
        .unwrap();

    handle_b.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Five jobs fire simultaneously ─────────────────────────────────────────────

/// Verify that the scheduler handles multiple jobs concurrently — all five
/// must fire within the same 5 s window.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_five_jobs_all_fire() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();

    const N: usize = 5;
    let ids: Vec<String> = (0..N).map(|i| unique_id(&format!("test-5jobs-{i}"))).collect();
    let subjects: Vec<String> = ids.iter().map(|id| format!("cron.fivejobs.{id}")).collect();

    // Create a subscriber for each job before starting the scheduler.
    let mut subs = Vec::new();
    for subject in &subjects {
        subs.push(cron_messages(&nats, &js, subject.clone()).await);
    }

    for (id, subject) in ids.iter().zip(subjects.iter()) {
        client.register_job(&JobConfig {
            id: id.clone(),
            schedule: Schedule::Interval { interval_sec: 1 },
            action: Action::Publish { subject: subject.clone() },
            enabled: true,
            payload: None,
        }).await.unwrap();
    }

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Every job must fire at least once within 5 s.
    for (i, sub) in subs.iter_mut().enumerate() {
        tokio::time::timeout(Duration::from_secs(5), sub.next())
            .await
            .unwrap_or_else(|_| panic!("Job #{i} never fired"))
            .unwrap()
            .unwrap();
    }

    handle.abort();
    for id in &ids {
        client.remove_job(id).await.unwrap();
    }
}

// ── Complex payload survives round-trip ──────────────────────────────────────

/// Verify that a deeply nested JSON payload is preserved bit-for-bit through
/// the full cycle: register → scheduler fires → TickPayload published → received.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_complex_payload_round_trip() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-complex-payload");
    let subject = format!("cron.complexpayload.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    let payload = serde_json::json!({
        "nested": {
            "count": 42,
            "tags": ["alpha", "beta", "gamma"],
            "active": true,
            "ratio": 3.14,
            "nothing": null
        },
        "top_level": "hello"
    });

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: Some(payload.clone()),
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("Timed out waiting for tick")
        .unwrap()
        .unwrap();

    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(tick.payload.unwrap(), payload, "payload must survive the full round-trip unchanged");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Remove and re-add job ─────────────────────────────────────────────────────

/// Verify the full remove → silence → re-add → resume cycle:
///   1. Job fires normally.
///   2. Job is removed — ticks stop.
///   3. Same job id is re-registered — ticks resume.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_remove_and_readd_job() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-readd");
    let subject = format!("cron.readd.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    let job = JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
    };

    client.register_job(&job).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Step 1: confirm it fires.
    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("Job never fired before removal")
        .unwrap()
        .unwrap();

    // Step 2: remove — ticks must stop.
    client.remove_job(&id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    let silence = tokio::time::timeout(Duration::from_secs(2), sub.next()).await;
    assert!(silence.is_err(), "Job should not fire after removal");

    // Step 3: re-add with the same id — ticks must resume.
    client.register_job(&job).await.unwrap();

    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("Job did not resume after being re-added")
        .unwrap()
        .unwrap();

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── All spawn env vars are injected correctly ─────────────────────────────────

/// Verify that ALL four tick context variables are injected as env vars into
/// the spawned process: `CRON_JOB_ID`, `CRON_FIRED_AT`, `CRON_EXECUTION_ID`,
/// and `CRON_PAYLOAD`.
///
/// The script writes each variable to its own temp file so we can assert
/// format and content independently.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_all_env_vars_injected() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-all-envvars");

    let tmp_dir = std::env::temp_dir();
    let f_job_id   = tmp_dir.join(format!("cron-envvar-job-id-{id}"));
    let f_fired_at = tmp_dir.join(format!("cron-envvar-fired-at-{id}"));
    let f_exec_id  = tmp_dir.join(format!("cron-envvar-exec-id-{id}"));
    let f_payload  = tmp_dir.join(format!("cron-envvar-payload-{id}"));

    for f in [&f_job_id, &f_fired_at, &f_exec_id, &f_payload] {
        let _ = std::fs::remove_file(f);
    }

    let script = format!(
        "printf '%s' \"$CRON_JOB_ID\"       > '{}' && \
         printf '%s' \"$CRON_FIRED_AT\"     > '{}' && \
         printf '%s' \"$CRON_EXECUTION_ID\" > '{}' && \
         printf '%s' \"$CRON_PAYLOAD\"      > '{}'",
        f_job_id.display(), f_fired_at.display(),
        f_exec_id.display(), f_payload.display(),
    );

    let custom_payload = serde_json::json!({ "env": "test", "value": 99 });

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: Some(custom_payload.clone()),
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Poll until all four files appear.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let all = [&f_job_id, &f_fired_at, &f_exec_id, &f_payload]
            .iter()
            .all(|f| f.exists());
        if all { break; }
        assert!(tokio::time::Instant::now() < deadline, "Timed out — not all env vars were written");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // CRON_JOB_ID must equal the job id.
    let job_id_val = std::fs::read_to_string(&f_job_id).unwrap();
    assert_eq!(job_id_val.trim(), id.as_str(), "CRON_JOB_ID mismatch");

    // CRON_FIRED_AT must be a valid RFC 3339 timestamp.
    let fired_at_val = std::fs::read_to_string(&f_fired_at).unwrap();
    chrono::DateTime::parse_from_rfc3339(fired_at_val.trim())
        .expect("CRON_FIRED_AT is not a valid RFC 3339 timestamp");

    // CRON_EXECUTION_ID must be a non-empty string (UUID).
    let exec_id_val = std::fs::read_to_string(&f_exec_id).unwrap();
    assert!(!exec_id_val.trim().is_empty(), "CRON_EXECUTION_ID must not be empty");

    // CRON_PAYLOAD must be valid JSON matching the configured payload.
    let payload_val = std::fs::read_to_string(&f_payload).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(payload_val.trim())
        .expect("CRON_PAYLOAD is not valid JSON");
    assert_eq!(parsed, custom_payload, "CRON_PAYLOAD content mismatch");

    handle.abort();
    client.remove_job(&id).await.unwrap();
    for f in [&f_job_id, &f_fired_at, &f_exec_id, &f_payload] {
        let _ = std::fs::remove_file(f);
    }
}

// ── Payload changes on hot-reload ─────────────────────────────────────────────

/// Verify that updating the payload field via hot-reload is reflected in the
/// very next tick: the scheduler discards the old payload and uses the new one.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_payload_changes_on_hot_reload() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-payload-reload");
    let subject = format!("cron.payloadreload.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    let payload_v1 = serde_json::json!({ "version": 1 });
    let payload_v2 = serde_json::json!({ "version": 2 });

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: Some(payload_v1.clone()),
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Step 1: first tick must carry v1 payload.
    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await.expect("First tick never arrived").unwrap().unwrap();
    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(tick.payload.unwrap(), payload_v1, "expected v1 payload on first tick");

    // Step 2: hot-update to v2 payload.
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: Some(payload_v2.clone()),
    }).await.unwrap();

    // Step 3: next tick must carry v2 payload.
    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await.expect("Second tick never arrived").unwrap().unwrap();
    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(tick.payload.unwrap(), payload_v2, "expected v2 payload after hot-reload");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Schedule change: interval → cron via hot-reload ──────────────────────────

/// Verify that a job can switch from `Schedule::Interval` to `Schedule::Cron`
/// at runtime via hot-reload, and the new schedule takes effect correctly.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_schedule_change_interval_to_cron() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-sched-change");
    let subject = format!("cron.schedchange.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    // Start with a slow interval (10 s) so the cron change is clearly observable.
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 10 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Interval jobs fire immediately — wait for the first tick.
    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await.expect("First tick never arrived").unwrap().unwrap();

    // Hot-reload: switch to cron expression every second.
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Cron { expr: "* * * * * *".to_string() },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
    }).await.unwrap();

    // The cron job must fire within 5 s — at 10 s interval it would take ~10 s.
    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await.expect("Cron schedule did not take effect after hot-reload").unwrap().unwrap();

    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(tick.job_id, id);

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── CLI binary: add spawn job → serve fires it ────────────────────────────────

/// Full end-to-end test using only the CLI binary:
///   1. `job add` registers a spawn job (touches a sentinel file).
///   2. `serve` starts the scheduler.
///   3. The scheduler fires the job and the sentinel file appears.
///   4. SIGTERM shuts the scheduler down cleanly.
#[cfg(unix)]
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_cli_spawn_job_fires() {
    let (nats, js) = connect_js().await;
    let id = unique_id("test-cli-spawn");
    let nats_url = test_url();
    let sentinel: PathBuf = std::env::temp_dir().join(format!("trogon-cli-spawn-{id}"));
    let _ = std::fs::remove_file(&sentinel);

    let bin = env!("CARGO_BIN_EXE_trogon-cron");

    // Write the spawn job JSON to a temp file.
    let tmp_json = std::env::temp_dir().join(format!("trogon-cli-spawn-{id}.json"));
    let job_json = serde_json::json!({
        "id": id,
        "schedule": { "type": "interval", "interval_sec": 1 },
        "action": {
            "type": "spawn",
            "bin": "/usr/bin/touch",
            "args": [sentinel.to_str().unwrap()],
            "concurrent": false,
            "timeout_sec": 5
        },
        "enabled": true
    }).to_string();
    std::fs::write(&tmp_json, &job_json).unwrap();

    // job add
    let out = tokio::process::Command::new(bin)
        .args(["--nats-url", &nats_url, "job", "add", tmp_json.to_str().unwrap()])
        .env("RUST_LOG", "error")
        .output().await.unwrap();
    assert!(out.status.success(), "job add failed: {}", String::from_utf8_lossy(&out.stderr));

    // Reset the leader lock so the binary becomes leader immediately.
    reset_leader_lock(&js).await;

    // serve — start the real binary scheduler.
    let mut child = tokio::process::Command::new(bin)
        .args(["serve"])
        .env("NATS_URL", &nats_url)
        .env("RUST_LOG", "error")
        .spawn()
        .expect("Failed to spawn trogon-cron serve");

    // Poll until the sentinel file appears (max 8 s).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        if sentinel.exists() { break; }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Spawn job never fired — sentinel file not created"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // SIGTERM — clean shutdown.
    let pid = child.id().expect("No PID");
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid as i32),
        nix::sys::signal::Signal::SIGTERM,
    ).expect("kill(SIGTERM) failed");
    tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await.expect("Binary did not exit after SIGTERM").unwrap();

    // Cleanup
    let client = CronClient::new(nats.clone()).await.unwrap();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&sentinel);
    let _ = std::fs::remove_file(&tmp_json);
}

// ── Ten jobs fire simultaneously ──────────────────────────────────────────────

/// Stress test: 10 jobs registered at once — all must fire within 5 s.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_ten_jobs_all_fire() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();

    const N: usize = 10;
    let ids: Vec<String> = (0..N).map(|i| unique_id(&format!("test-10jobs-{i}"))).collect();
    let subjects: Vec<String> = ids.iter().map(|id| format!("cron.tenjobs.{id}")).collect();

    let mut subs = Vec::new();
    for subject in &subjects {
        subs.push(cron_messages(&nats, &js, subject.clone()).await);
    }

    for (id, subject) in ids.iter().zip(subjects.iter()) {
        client.register_job(&JobConfig {
            id: id.clone(),
            schedule: Schedule::Interval { interval_sec: 1 },
            action: Action::Publish { subject: subject.clone() },
            enabled: true,
            payload: None,
        }).await.unwrap();
    }

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    for (i, sub) in subs.iter_mut().enumerate() {
        tokio::time::timeout(Duration::from_secs(5), sub.next())
            .await
            .unwrap_or_else(|_| panic!("Job #{i} out of {N} never fired"))
            .unwrap()
            .unwrap();
    }

    handle.abort();
    for id in &ids {
        client.remove_job(id).await.unwrap();
    }
}

// ── Disable while spawn is running stops re-entry ────────────────────────────

/// Verify that disabling a job while a spawned process is still running
/// prevents new invocations from starting, but does NOT kill the existing one.
///
/// Strategy: spawn a long-running process (sleep 10 s) that appends to a
/// counter on start.  After the first invocation begins, disable the job.
/// After 3 more seconds (≥ 3 ticks at interval=1 s) the counter must still
/// show exactly 1 — no new instance started.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_disable_stops_spawn_re_entry() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-disable-spawn");
    let counter: PathBuf = std::env::temp_dir().join(format!("trogon-cron-disablespawn-{id}"));
    let _ = std::fs::remove_file(&counter);

    // Each invocation appends one line then sleeps 10 s.
    let script = format!("echo x >> '{}'; sleep 10", counter.to_str().unwrap());

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script],
            concurrent: false,
            timeout_sec: Some(15),
        },
        enabled: true,
        payload: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Wait for the first invocation to start.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if counter.exists() { break; }
        assert!(tokio::time::Instant::now() < deadline, "First invocation never started");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Disable the job — scheduler hot-reloads, no new invocations must start.
    client.set_enabled(&id, false).await.unwrap();

    // Wait 3 more seconds (≥ 3 ticks at interval=1 s).
    tokio::time::sleep(Duration::from_secs(3)).await;

    let line_count = std::fs::read_to_string(&counter)
        .unwrap_or_default()
        .lines()
        .count();

    assert_eq!(
        line_count, 1,
        "disabling the job must prevent new invocations; got {line_count}"
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&counter);
}
