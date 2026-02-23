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
