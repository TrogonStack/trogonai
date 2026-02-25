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
    // Use a full UUID v4 segment so IDs are globally unique across test runs.
    // subsec_nanos would repeat on back-to-back runs (same nanosecond within a second),
    // causing different test invocations to share the same KV key and counter file.
    let short = uuid::Uuid::new_v4()
        .to_string()
        .split('-')
        .next()
        .unwrap()
        .to_string();
    format!("{prefix}-{short}")
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

/// Remove every job currently in the cron_configs bucket.
///
/// Called at the start of Spawn-based scheduler tests so that stale entries
/// from previous (possibly failing) test runs do not interfere: without this,
/// the scheduler loads all leftover jobs, spawns processes for each, and the
/// sheer number of initial-snapshot entries can push the `load_jobs_and_watch`
/// 500 ms deadline before the current test's job is delivered.
async fn purge_all_jobs(client: &trogon_cron::CronClient) {
    if let Ok(jobs) = client.list_jobs().await {
        for job in jobs {
            let _ = client.remove_job(&job.id).await;
        }
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
}).await.unwrap();

    client.register_job(&JobConfig {
        id: id_b.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject_b.clone() },
        enabled: true,
        payload: None,
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
}).await.unwrap();

    // Healthy publish job — must fire despite the failing spawn.
    client.register_job(&JobConfig {
        id: id_ok.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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
        retry: None,
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

// ── SIGTERM ignored → SIGKILL escalation ─────────────────────────────────────

/// Verify the SIGKILL escalation path in `kill_gracefully`:
/// when a spawned process ignores SIGTERM, the scheduler escalates to SIGKILL
/// after a 5 s grace period.
///
/// The process uses `trap '' TERM` to ignore SIGTERM.  With `timeout_sec: 2`
/// the scheduler fires SIGTERM (ignored), waits 5 s, then fires SIGKILL.
/// The sentinel "done" file must never be created because the process is
/// killed before the sleep completes.
#[cfg(unix)]
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_sigkill_escalation() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-sigkill");
    let started: PathBuf = std::env::temp_dir().join(format!("trogon-sigkill-started-{id}"));
    let done: PathBuf    = std::env::temp_dir().join(format!("trogon-sigkill-done-{id}"));
    let _ = std::fs::remove_file(&started);
    let _ = std::fs::remove_file(&done);

    // trap '' TERM makes the process ignore SIGTERM.
    // It writes "started", sleeps 60 s, then writes "done".
    // With timeout_sec=2: SIGTERM fires at t=2, is ignored, SIGKILL fires at t=7.
    let script = format!(
        "trap '' TERM; touch '{}'; sleep 60; touch '{}'",
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
            timeout_sec: Some(2), // SIGTERM at t=2, SIGKILL at t=2+5=7
        },
        enabled: true,
        payload: None,
        retry: None,
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

    // Wait for SIGTERM (2 s) + SIGTERM grace (5 s) + margin (2 s) = 9 s total.
    tokio::time::sleep(Duration::from_secs(9)).await;

    // SIGKILL must have killed the process — "done" must not exist.
    assert!(!done.exists(), "Process completed despite SIGKILL — escalation did not work");

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&started);
    let _ = std::fs::remove_file(&done);
}

// ── is_running preserved across hot-reload ───────────────────────────────────

/// Verify that hot-reloading a spawn job's config while a process is still
/// running does NOT reset the `is_running` flag.
///
/// The scheduler explicitly clones `is_running` from the old state on
/// hot-reload (scheduler.rs handle_config_change).  If this were broken, the
/// reloaded job would start a second overlapping process despite
/// `concurrent: false`.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_concurrent_false_preserved_across_hot_reload() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();

    // Remove all stale jobs from previous (possibly failing) runs so the scheduler
    // starts with a clean slate and loads_jobs_and_watch completes within its 500 ms
    // deadline (many leftover jobs can push the snapshot delivery past the deadline).
    purge_all_jobs(&client).await;

    let id = unique_id("test-running-reload");
    let counter: PathBuf = std::env::temp_dir().join(format!("trogon-running-reload-{id}"));
    let _ = std::fs::remove_file(&counter);

    // Each invocation appends a line then sleeps 10 s.
    let script = format!("echo x >> '{}'; sleep 10", counter.to_str().unwrap());

    let base_job = JobConfig {
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
        retry: None,
};

    client.register_job(&base_job).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Wait for the first process to start.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if counter.exists() { break; }
        assert!(tokio::time::Instant::now() < deadline, "First invocation never started");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Hot-reload the job (change payload) while process is still running.
    client.register_job(&JobConfig {
        payload: Some(serde_json::json!({ "reloaded": true })),
        ..base_job
    }).await.unwrap();

    // Wait 3 more seconds (≥ 3 ticks).  The reload must NOT reset is_running.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let line_count = std::fs::read_to_string(&counter)
        .unwrap_or_default()
        .lines()
        .count();

    assert_eq!(
        line_count, 1,
        "is_running must be preserved across hot-reload; got {line_count} invocations"
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&counter);
}

// ── Action type change: Spawn → Publish via hot-reload ────────────────────────

/// Verify that a job can change its action type from `Spawn` to `Publish` at
/// runtime, and the scheduler correctly switches to publishing NATS messages.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_action_change_spawn_to_publish() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-action-change");
    let subject = format!("cron.actionchange.{id}");
    let counter: PathBuf = std::env::temp_dir().join(format!("trogon-action-change-{id}"));
    let _ = std::fs::remove_file(&counter);

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    // Start with a spawn job.
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
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Wait for the spawn to fire at least once.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if counter.exists() { break; }
        assert!(tokio::time::Instant::now() < deadline, "Spawn job never fired");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Hot-reload: switch action to Publish.
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    // A NATS tick must now arrive — the scheduler switched to publishing.
    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("No NATS tick after switching action from Spawn to Publish")
        .unwrap()
        .unwrap();

    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(tick.job_id, id);

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&counter);
}

// ── Remove non-existent job is a no-op ───────────────────────────────────────

/// Verify that calling `remove_job` on a job id that was never registered
/// succeeds without error (the operation is idempotent / no-op).
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_remove_nonexistent_job_is_noop() {
    let nats = connect().await;
    let client = CronClient::new(nats).await.unwrap();
    let id = unique_id("test-remove-ghost");

    // The job was never registered — remove_job must not return an error.
    client.remove_job(&id).await
        .expect("remove_job on a non-existent id should succeed silently");

    // Calling it a second time must also succeed.
    client.remove_job(&id).await
        .expect("second remove_job on same non-existent id should also succeed");
}

// ── Non-existent binary in KV is skipped, scheduler continues ─────────────────

/// Verify that a spawn job whose binary does not exist is silently skipped by
/// the scheduler (via build_job_state validation), while other valid jobs
/// continue to fire normally.
///
/// The `CronClient` does not check binary existence (the client may run on a
/// different host), so `register_job` accepts the path.  The scheduler then
/// rejects it at load/hot-reload time via `validate_spawn_config`.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_nonexistent_bin_skipped_scheduler_continues() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id_bad = unique_id("test-bad-bin");
    let id_ok  = unique_id("test-bad-bin-ok");
    let subject = format!("cron.badbin.{id_ok}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    // Healthy publish job.
    client.register_job(&JobConfig {
        id: id_ok.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Confirm the healthy job fires before injecting the bad entry.
    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await.expect("Healthy job never fired").unwrap().unwrap();

    // Register a spawn job whose binary does not exist.
    // register_job accepts it (no existence check on the client side).
    // The scheduler rejects it via validate_spawn_config and skips it.
    client.register_job(&JobConfig {
        id: id_bad.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/nonexistent/binary-that-does-not-exist".to_string(),
            args: vec![],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    // The healthy publish job must keep firing — bad binary must not crash scheduler.
    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("Scheduler stalled after registering a spawn job with non-existent binary")
        .unwrap()
        .unwrap();

    handle.abort();
    client.remove_job(&id_bad).await.unwrap();
    client.remove_job(&id_ok).await.unwrap();
}

// ── Registering same id twice overwrites the entry ────────────────────────────

/// Verify that registering a job with an already-used id replaces the previous
/// entry in NATS KV.  `list_jobs` must return exactly one job for that id,
/// with the config from the second registration.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_register_same_id_overwrites() {
    let nats = connect().await;
    let client = CronClient::new(nats).await.unwrap();
    let id = unique_id("test-overwrite");

    let job_v1 = JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 60 },
        action: Action::Publish { subject: "cron.overwrite.v1".to_string() },
        enabled: true,
        payload: Some(serde_json::json!({ "version": 1 })),
        retry: None,
};
    let job_v2 = JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 120 },
        action: Action::Publish { subject: "cron.overwrite.v2".to_string() },
        enabled: false,
        payload: Some(serde_json::json!({ "version": 2 })),
        retry: None,
};

    client.register_job(&job_v1).await.unwrap();
    client.register_job(&job_v2).await.unwrap(); // must overwrite v1

    // Only one entry for this id must exist.
    let jobs = client.list_jobs().await.unwrap();
    let matches: Vec<_> = jobs.iter().filter(|j| j.id == id).collect();
    assert_eq!(matches.len(), 1, "duplicate entries found for the same job id");

    // The stored entry must reflect v2.
    let stored = client.get_job(&id).await.unwrap().expect("job must exist");
    assert_eq!(stored.payload.unwrap()["version"], 2, "stored job must be v2");
    assert!(!stored.enabled, "stored job must have v2 enabled=false");

    client.remove_job(&id).await.unwrap();
}

// ── set_enabled on non-existent job returns error ────────────────────────────

/// Verify that calling `set_enabled` on a job id that was never registered
/// returns an error rather than silently succeeding.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_set_enabled_nonexistent_job_fails() {
    let nats = connect().await;
    let client = CronClient::new(nats).await.unwrap();
    let id = unique_id("test-setenabled-ghost");

    let result = client.set_enabled(&id, true).await;
    assert!(result.is_err(), "set_enabled on a non-existent job must return an error");
}

// ── Spawn with multiple positional arguments ──────────────────────────────────

/// Verify that multiple positional arguments in `Action::Spawn::args` are all
/// passed through to the spawned process correctly.
///
/// Uses `/bin/sh -c '...' _ alpha beta gamma` so $1=$alpha, $2=beta, $3=gamma.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_multiple_positional_args() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-multi-args");
    let output: PathBuf = std::env::temp_dir().join(format!("trogon-multi-args-{id}"));
    let _ = std::fs::remove_file(&output);

    let script = format!(
        "printf '%s-%s-%s' \"$1\" \"$2\" \"$3\" > '{}'",
        output.to_str().unwrap()
    );

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                script,
                "_".to_string(),     // $0 placeholder
                "alpha".to_string(), // $1
                "beta".to_string(),  // $2
                "gamma".to_string(), // $3
            ],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if output.exists() { break; }
        assert!(tokio::time::Instant::now() < deadline, "Process never wrote output");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let content = std::fs::read_to_string(&output).unwrap();
    assert_eq!(
        content.trim(), "alpha-beta-gamma",
        "all three positional args must be passed to the spawned process"
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&output);
}

// ── Cron schedule timing accuracy ─────────────────────────────────────────────

/// Verify that a `*/2 * * * * *` cron job fires at ~2 s intervals.
///
/// Collects three consecutive ticks and checks that the gap between each
/// pair of `fired_at` timestamps is between 1 s and 4 s (generous bounds
/// to accommodate CI timing variability).
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_cron_schedule_timing_accuracy() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-cron-timing");
    let subject = format!("cron.timing.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Cron { expr: "*/2 * * * * *".to_string() },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let mut timestamps = Vec::new();
    for i in 0..3 {
        let msg = tokio::time::timeout(Duration::from_secs(6), sub.next())
            .await
            .unwrap_or_else(|_| panic!("Timed out on tick #{}", i + 1))
            .unwrap()
            .unwrap();
        let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
        timestamps.push(tick.fired_at);
    }

    // Gap between consecutive ticks must be roughly 2 s (1 – 4 s).
    for i in 0..2 {
        let gap_ms = (timestamps[i + 1] - timestamps[i])
            .num_milliseconds()
            .unsigned_abs();
        assert!(
            gap_ms >= 1_000 && gap_ms <= 4_000,
            "gap between tick #{} and #{} was {} ms, expected ~2000 ms",
            i + 1, i + 2, gap_ms
        );
    }

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Change concurrent: false → true via hot-reload ───────────────────────────

/// Verify that changing `concurrent` from `false` to `true` via hot-reload
/// immediately allows overlapping invocations.
///
/// After the change, new ticks must start new processes even while earlier
/// ones are still sleeping.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_concurrent_setting_change_false_to_true() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-concurrent-change");
    let counter: PathBuf = std::env::temp_dir().join(format!("trogon-concurrent-change-{id}"));
    let _ = std::fs::remove_file(&counter);

    let script = format!("echo x >> '{}'; sleep 5", counter.to_str().unwrap());

    // Start with concurrent=false.
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script.clone()],
            concurrent: false,
            timeout_sec: Some(10),
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Wait for the first invocation (concurrent=false, process sleeping).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if counter.exists() { break; }
        assert!(tokio::time::Instant::now() < deadline, "First invocation never started");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let count_before = std::fs::read_to_string(&counter)
        .unwrap_or_default().lines().count();
    assert_eq!(count_before, 1, "expected exactly 1 invocation before hot-reload");

    // Hot-reload: switch to concurrent=true.
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
        retry: None,
}).await.unwrap();

    // After 3 s, new invocations must have started despite the first still sleeping.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let count_after = std::fs::read_to_string(&counter)
        .unwrap_or_default().lines().count();

    assert!(
        count_after > 1,
        "switching to concurrent=true must allow overlapping invocations; got {count_after}"
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&counter);
}

// ── Schedule type change: cron → interval via hot-reload ─────────────────────

/// Hot-reload from a low-frequency cron expression (fires every 30 s) to a
/// 1-second interval. The scheduler must switch to the new schedule without
/// restarting, and the first tick must arrive within 3 s of the reload.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_cron_to_interval_hot_reload() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-cron-to-interval");
    let subject = format!("cron.reload-cron2interval.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    // Register with a 30-second cron expression — won't fire during the test.
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Cron { expr: "*/30 * * * * *".to_string() },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // No tick must arrive in the first 2 s under */30 schedule.
    let no_tick = tokio::time::timeout(Duration::from_secs(2), sub.next()).await;
    assert!(no_tick.is_err(), "Job must not fire yet under */30 cron expression");

    // Hot-reload to interval_sec=1.
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    // Must now fire within 3 s.
    let msg = tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await
        .expect("Timed out waiting for tick after cron->interval hot-reload")
        .unwrap()
        .unwrap();
    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(tick.job_id, id);

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Two concurrent:false jobs have independent is_running flags ───────────────

/// Two spawn jobs both with `concurrent: false`. Job A sleeps 20 s (holds its
/// own `is_running` guard). Job B must still fire independently because the
/// two guards are stored per-job, not shared.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_two_concurrent_false_jobs_independent_flags() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id_a = unique_id("test-conc-flag-a");
    let id_b = unique_id("test-conc-flag-b");
    let out_a: PathBuf = std::env::temp_dir().join(format!("trogon-conc-a-{id_a}"));
    let out_b: PathBuf = std::env::temp_dir().join(format!("trogon-conc-b-{id_b}"));
    let _ = std::fs::remove_file(&out_a);
    let _ = std::fs::remove_file(&out_b);

    // Job A: writes a marker then sleeps 20 s, blocking its own guard.
    client.register_job(&JobConfig {
        id: id_a.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(),
                format!("echo x > '{}'; sleep 20", out_a.to_str().unwrap())],
            concurrent: false,
            timeout_sec: Some(30),
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    // Job B: just appends a marker quickly.
    client.register_job(&JobConfig {
        id: id_b.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(),
                format!("echo x >> '{}'", out_b.to_str().unwrap())],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Wait until job A has started (is_running=true, sleeping).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if out_a.exists() { break; }
        assert!(tokio::time::Instant::now() < deadline, "Job A never started");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Give job B time to run while job A is still sleeping.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let b_count = std::fs::read_to_string(&out_b)
        .unwrap_or_default().lines().count();
    assert!(
        b_count >= 1,
        "Job B must run independently while Job A holds its is_running guard; got {b_count} invocations"
    );

    handle.abort();
    client.remove_job(&id_a).await.unwrap();
    client.remove_job(&id_b).await.unwrap();
    let _ = std::fs::remove_file(&out_a);
    let _ = std::fs::remove_file(&out_b);
}

// ── Multiple rapid hot-reloads settle on the final config ────────────────────

/// Register a job then immediately hot-reload it four more times (five total),
/// each with a different publish subject. Only the last subject must fire;
/// earlier subjects must remain silent after being superseded.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_multiple_rapid_hot_reloads() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-rapid-reloads");

    let subjects: Vec<String> = (1..=5)
        .map(|i| format!("cron.rapid-reload-{i}.{id}"))
        .collect();

    let mut subs = Vec::new();
    for subject in &subjects {
        subs.push(cron_messages(&nats, &js, subject.clone()).await);
    }

    // Register through all 5 subjects in rapid succession.
    for subject in &subjects {
        client.register_job(&JobConfig {
            id: id.clone(),
            schedule: Schedule::Interval { interval_sec: 1 },
            action: Action::Publish { subject: subject.clone() },
            enabled: true,
            payload: None,
        retry: None,
}).await.unwrap();
    }

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Final subject must fire within 3 s.
    let msg = tokio::time::timeout(Duration::from_secs(3), subs[4].next())
        .await
        .expect("Timed out waiting for tick on final subject after rapid reloads")
        .unwrap()
        .unwrap();
    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(tick.job_id, id, "tick must carry the correct job id");

    // Earlier subjects must not have received ticks.
    for i in 0..4 {
        let stale = tokio::time::timeout(Duration::from_millis(300), subs[i].next()).await;
        assert!(
            stale.is_err(),
            "Subject {} must not receive ticks after being replaced",
            subjects[i]
        );
    }

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Spawn receives CRON_PAYLOAD environment variable ─────────────────────────

/// When `JobConfig.payload` is set, the spawned child process must receive its
/// value in the `CRON_PAYLOAD` environment variable.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_payload_env_var() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-spawn-payload-env");
    let output: PathBuf = std::env::temp_dir().join(format!("trogon-payload-env-{id}"));
    let _ = std::fs::remove_file(&output);

    let payload_value = format!("hello-payload-{id}");
    let payload_json = serde_json::Value::String(payload_value.clone());

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                format!("printf '%s' \"$CRON_PAYLOAD\" > '{}'", output.to_str().unwrap()),
            ],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: Some(payload_json),
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if output.exists() {
            let content = std::fs::read_to_string(&output).unwrap();
            if !content.is_empty() { break; }
        }
        assert!(tokio::time::Instant::now() < deadline, "Spawn job never wrote output");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // CRON_PAYLOAD is JSON-serialized, so a string value is wrapped in quotes.
    let written = std::fs::read_to_string(&output).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(written.trim())
        .expect("CRON_PAYLOAD was not valid JSON");
    assert_eq!(
        parsed.as_str().unwrap(), payload_value,
        "CRON_PAYLOAD env var must match the configured payload"
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&output);
}

// ── Remove then re-add a job with a different publish subject ─────────────────

/// Remove a running job then re-register it under a new publish subject.
/// The old subject must stop receiving ticks and the new subject must start.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_remove_then_readd_job() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-remove-readd");
    let subject_old = format!("cron.remove-readd-old.{id}");
    let subject_new = format!("cron.remove-readd-new.{id}");

    let mut sub_old = cron_messages(&nats, &js, subject_old.clone()).await;
    let mut sub_new = cron_messages(&nats, &js, subject_new.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject_old.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Old subject must fire first.
    tokio::time::timeout(Duration::from_secs(3), sub_old.next())
        .await
        .expect("Timed out waiting for initial tick on old subject")
        .unwrap()
        .unwrap();

    // Remove, then re-register under a new subject.
    client.remove_job(&id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject_new.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    // New subject must fire.
    let msg = tokio::time::timeout(Duration::from_secs(3), sub_new.next())
        .await
        .expect("Timed out waiting for tick on new subject after re-add")
        .unwrap()
        .unwrap();
    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(tick.job_id, id);

    // Old subject must not receive any further ticks.
    let stale = tokio::time::timeout(Duration::from_millis(500), sub_old.next()).await;
    assert!(stale.is_err(), "Old subject must not fire after job was removed and re-added");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── execution_id is a unique UUID per tick ────────────────────────────────────

/// Collect five consecutive ticks on the same interval job and assert that
/// every `execution_id` is non-empty and distinct from all others.
/// Proves that the scheduler generates a fresh UUID for each individual tick.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_execution_id_unique_per_tick() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-exec-id-unique");
    let subject = format!("cron.exec-id.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let mut seen_ids = std::collections::HashSet::new();
    for i in 0..5 {
        let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
            .await
            .unwrap_or_else(|_| panic!("Timed out on tick #{}", i + 1))
            .unwrap()
            .unwrap();
        let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
        assert!(!tick.execution_id.is_empty(), "execution_id on tick #{} must not be empty", i + 1);
        let fresh = seen_ids.insert(tick.execution_id.clone());
        assert!(fresh, "duplicate execution_id on tick #{}: {}", i + 1, tick.execution_id);
    }

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── get_job reflects hot-reload immediately ───────────────────────────────────

/// Register a job, read it back with `get_job`, then hot-reload with a new
/// payload and read again. The second call must return the updated config.
/// Remove the job and verify `get_job` returns `None`.
/// No scheduler is needed — this is a pure client-side consistency check.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_get_job_reflects_hot_reload() {
    let nats = connect().await;
    let client = CronClient::new(nats).await.unwrap();
    let id = unique_id("test-get-reflects-reload");

    let payload_v1 = serde_json::json!({ "version": 1 });
    let payload_v2 = serde_json::json!({ "version": 2 });
    let subject = format!("cron.get-reload.{id}");

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 60 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: Some(payload_v1.clone()),
        retry: None,
    }).await.unwrap();

    let job = client.get_job(&id).await.unwrap().expect("job must exist after register");
    assert_eq!(job.payload.unwrap(), payload_v1, "initial get_job must return v1 payload");

    // Hot-reload with v2 payload.
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 60 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: Some(payload_v2.clone()),
        retry: None,
    }).await.unwrap();

    let updated = client.get_job(&id).await.unwrap().expect("job must still exist after reload");
    assert_eq!(updated.payload.unwrap(), payload_v2, "get_job must return v2 payload after hot-reload");

    // Remove and verify it disappears.
    client.remove_job(&id).await.unwrap();
    let gone = client.get_job(&id).await.unwrap();
    assert!(gone.is_none(), "get_job must return None after removal");
}

// ── Removing one job does not affect the others ───────────────────────────────

/// Register three independent jobs (A, B, C). Wait for all to fire at least
/// once, then remove B. After removal A and C must continue firing while B
/// goes completely silent.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_partial_removal_others_continue_firing() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id_a = unique_id("test-partial-rm-a");
    let id_b = unique_id("test-partial-rm-b");
    let id_c = unique_id("test-partial-rm-c");
    let sub_a_str = format!("cron.partial-rm-a.{id_a}");
    let sub_b_str = format!("cron.partial-rm-b.{id_b}");
    let sub_c_str = format!("cron.partial-rm-c.{id_c}");

    let mut sub_a = cron_messages(&nats, &js, sub_a_str.clone()).await;
    let mut sub_b = cron_messages(&nats, &js, sub_b_str.clone()).await;
    let mut sub_c = cron_messages(&nats, &js, sub_c_str.clone()).await;

    for (id, subject) in [(&id_a, &sub_a_str), (&id_b, &sub_b_str), (&id_c, &sub_c_str)] {
        client.register_job(&JobConfig {
            id: id.clone(),
            schedule: Schedule::Interval { interval_sec: 1 },
            action: Action::Publish { subject: subject.clone() },
            enabled: true,
            payload: None,
        retry: None,
}).await.unwrap();
    }

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // All three must fire at least once.
    for sub in [&mut sub_a, &mut sub_b, &mut sub_c] {
        tokio::time::timeout(Duration::from_secs(5), sub.next())
            .await
            .expect("Timed out waiting for initial tick")
            .unwrap()
            .unwrap();
    }

    // Remove B.
    client.remove_job(&id_b).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // A and C must keep firing.
    tokio::time::timeout(Duration::from_secs(3), sub_a.next())
        .await
        .expect("Job A must continue firing after B was removed")
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), sub_c.next())
        .await
        .expect("Job C must continue firing after B was removed")
        .unwrap()
        .unwrap();

    // B must be silent.
    let stale = tokio::time::timeout(Duration::from_millis(500), sub_b.next()).await;
    assert!(stale.is_err(), "Job B must not fire after removal");

    handle.abort();
    client.remove_job(&id_a).await.unwrap();
    // id_b already removed
    client.remove_job(&id_c).await.unwrap();
}

// ── list_jobs tracks additions and removals ───────────────────────────────────

/// Register three jobs with unique IDs, confirm all appear in `list_jobs`.
/// Remove one, confirm it disappears. Remove the rest, confirm they are gone.
/// No scheduler required — this is a pure `CronClient` API test.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_list_jobs_tracks_add_remove() {
    let nats = connect().await;
    let client = CronClient::new(nats).await.unwrap();
    let id1 = unique_id("test-list-track-1");
    let id2 = unique_id("test-list-track-2");
    let id3 = unique_id("test-list-track-3");
    let ids = [id1.clone(), id2.clone(), id3.clone()];

    for id in &ids {
        client.register_job(&JobConfig {
            id: id.clone(),
            schedule: Schedule::Interval { interval_sec: 60 },
            action: Action::Publish { subject: format!("cron.list-track.{id}") },
            enabled: true,
            payload: None,
        retry: None,
}).await.unwrap();
    }

    let list = client.list_jobs().await.unwrap();
    let listed_ids: Vec<&str> = list.iter().map(|j| j.id.as_str()).collect();
    for id in &ids {
        assert!(listed_ids.contains(&id.as_str()), "id {id} must appear in list_jobs after register");
    }

    // Remove the middle job.
    client.remove_job(&id2).await.unwrap();
    let list = client.list_jobs().await.unwrap();
    let listed_ids: Vec<&str> = list.iter().map(|j| j.id.as_str()).collect();
    assert!(!listed_ids.contains(&id2.as_str()), "id2 must not appear after removal");
    assert!(listed_ids.contains(&id1.as_str()), "id1 must still appear");
    assert!(listed_ids.contains(&id3.as_str()), "id3 must still appear");

    // Remove the remaining two.
    client.remove_job(&id1).await.unwrap();
    client.remove_job(&id3).await.unwrap();
    let list = client.list_jobs().await.unwrap();
    let listed_ids: Vec<&str> = list.iter().map(|j| j.id.as_str()).collect();
    for id in &ids {
        assert!(!listed_ids.contains(&id.as_str()), "id {id} must not appear after removal");
    }
}

// ── fired_at timestamp is close to real wall-clock time ──────────────────────

/// Verify that `fired_at` in the tick payload is within the observation
/// window: it must not predate the moment the test registered the job, and
/// must not postdate the moment the message was received.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_fired_at_is_recent() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-fired-at-recent");
    let subject = format!("cron.fired-at.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    let before = chrono::Utc::now();

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
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
    let after = chrono::Utc::now();

    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();

    assert!(
        tick.fired_at >= before,
        "fired_at ({}) must not predate test start ({})",
        tick.fired_at, before
    );
    // Allow 1 s of clock skew between scheduler and test process.
    assert!(
        tick.fired_at <= after + chrono::Duration::seconds(1),
        "fired_at ({}) must not be far in the future (after={})",
        tick.fired_at, after
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Spawn job triggered by a cron expression ─────────────────────────────────

/// Verify the cron-expression + Action::Spawn combination end-to-end.
///
/// All other spawn tests use `Schedule::Interval`.  This test uses
/// `*/2 * * * * *` (every 2 s) to prove the scheduler also invokes binaries
/// when the trigger is a cron expression instead of an interval.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_triggered_by_cron_expression() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-spawn-cron-expr");
    let output: PathBuf = std::env::temp_dir().join(format!("trogon-spawn-cron-{id}"));
    let _ = std::fs::remove_file(&output);

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Cron { expr: "*/2 * * * * *".to_string() },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                format!("echo x >> '{}'", output.to_str().unwrap()),
            ],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Cron fires at 2-s boundaries; within 6 s at least one invocation must run.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    loop {
        if output.exists() { break; }
        assert!(tokio::time::Instant::now() < deadline,
            "Spawn job triggered by cron expression never fired");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&output);
}

// ── is_running clears after a timeout kill ────────────────────────────────────

/// Register a `concurrent: false` spawn that sleeps longer than its timeout.
/// After the scheduler kills the first invocation (timeout + graceful kill),
/// the `is_running` guard must be cleared and the job must fire a second time.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_is_running_clears_after_timeout_kill() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-is-running-clear");
    let counter: PathBuf = std::env::temp_dir().join(format!("trogon-refire-{id}"));
    let _ = std::fs::remove_file(&counter);

    // Each invocation appends a line then sleeps — far longer than the timeout.
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(),
                format!("echo x >> '{}'; sleep 30", counter.to_str().unwrap())],
            concurrent: false,
            timeout_sec: Some(2), // killed after 2 s
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Wait for the first invocation.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if counter.exists() { break; }
        assert!(tokio::time::Instant::now() < deadline, "First invocation never started");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // After timeout (2 s) + SIGTERM grace period the process exits and
    // is_running is cleared.  Wait up to 12 s for a second invocation.
    let deadline2 = tokio::time::Instant::now() + Duration::from_secs(12);
    loop {
        let count = std::fs::read_to_string(&counter)
            .unwrap_or_default().lines().count();
        if count >= 2 { break; }
        assert!(tokio::time::Instant::now() < deadline2,
            "is_running was not cleared after timeout kill; second invocation never started");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&counter);
}

// ── Three schedulers — exactly one acts as leader ─────────────────────────────

/// Start three scheduler instances simultaneously.  Over a 5-second window
/// with a 1-second interval job, a single leader fires ~5 ticks; if all three
/// fired independently we would see ~15.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_three_schedulers_single_fires() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-three-sched");
    let subject = format!("cron.three-sched.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let (nats_a, nats_b, nats_c) = (nats.clone(), nats.clone(), nats.clone());
    let handle_a = tokio::spawn(async move { Scheduler::new(nats_a).run().await.ok(); });
    let handle_b = tokio::spawn(async move { Scheduler::new(nats_b).run().await.ok(); });
    let handle_c = tokio::spawn(async move { Scheduler::new(nats_c).run().await.ok(); });

    let mut count = 0usize;
    let window = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(window);
    loop {
        tokio::select! {
            biased;
            _ = &mut window => break,
            msg = sub.next() => { if msg.is_some() { count += 1; } }
        }
    }

    // Single leader → ~5 ticks. Triple firing → ~15 ticks.
    assert!(count >= 3, "too few ticks — no scheduler became leader: {count}");
    assert!(count <= 8, "too many ticks — multiple schedulers are double-firing: {count}");

    handle_a.abort();
    handle_b.abort();
    handle_c.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Spawn with an empty args list ────────────────────────────────────────────

/// Verify that `Action::Spawn` works when `args` is empty — the binary is
/// invoked with no extra arguments.  A small self-contained executable script
/// is written to a temp file, made executable, and used as the target binary.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_empty_args() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-spawn-empty-args");

    // Write a self-contained script that creates a sentinel file when run.
    let sentinel: PathBuf = std::env::temp_dir().join(format!("trogon-empty-args-sentinel-{id}"));
    let script: PathBuf = std::env::temp_dir().join(format!("trogon-empty-args-script-{id}.sh"));
    let _ = std::fs::remove_file(&sentinel);

    std::fs::write(&script, format!(
        "#!/bin/sh\necho x > '{}'\n", sentinel.to_str().unwrap()
    )).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: script.to_str().unwrap().to_string(),
            args: vec![], // intentionally empty
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if sentinel.exists() { break; }
        assert!(tokio::time::Instant::now() < deadline,
            "Spawn job with empty args never fired");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&sentinel);
    let _ = std::fs::remove_file(&script);
}

// ── set_enabled is immediately reflected by get_job ──────────────────────────

/// Verify that `CronClient::set_enabled` updates the stored config atomically:
/// calling `get_job` right after must return the new `enabled` value.
/// No scheduler required — this is a pure client API consistency check.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_set_enabled_reflected_by_get_job() {
    let nats = connect().await;
    let client = CronClient::new(nats).await.unwrap();
    let id = unique_id("test-set-enabled-get");

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 60 },
        action: Action::Publish { subject: format!("cron.set-enabled.{id}") },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    let job = client.get_job(&id).await.unwrap().unwrap();
    assert!(job.enabled, "newly registered job must be enabled");

    client.set_enabled(&id, false).await.unwrap();
    let job = client.get_job(&id).await.unwrap().unwrap();
    assert!(!job.enabled, "get_job must reflect disabled state after set_enabled(false)");

    client.set_enabled(&id, true).await.unwrap();
    let job = client.get_job(&id).await.unwrap().unwrap();
    assert!(job.enabled, "get_job must reflect enabled state after set_enabled(true)");

    client.remove_job(&id).await.unwrap();
}

// ── Client rejects relative bin path ─────────────────────────────────────────

/// `register_job` must immediately return `InvalidJobConfig` when `Action::Spawn`
/// carries a relative (non-absolute) `bin` path.  Callers must get a clear error
/// rather than waiting for the scheduler to silently skip the job.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_register_job_rejects_relative_bin() {
    let nats = connect().await;
    let client = CronClient::new(nats).await.unwrap();

    let result = client.register_job(&JobConfig {
        id: unique_id("bad-bin-path"),
        schedule: Schedule::Interval { interval_sec: 60 },
        action: Action::Spawn {
            bin: "relative/path/to/bin".to_string(),
            args: vec![],
            concurrent: false,
            timeout_sec: None,
        },
        enabled: true,
        payload: None,
        retry: None,
}).await;

    assert!(result.is_err(), "register_job must reject a relative bin path");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("absolute"),
        "error message must mention 'absolute', got: {err}"
    );
}

// ── Client rejects timeout_sec = 0 ───────────────────────────────────────────

/// `register_job` must return an error when `timeout_sec` is `Some(0)`.
/// A zero-second timeout would kill every process immediately and is almost
/// certainly a configuration mistake.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_register_job_rejects_timeout_sec_zero() {
    let nats = connect().await;
    let client = CronClient::new(nats).await.unwrap();

    let result = client.register_job(&JobConfig {
        id: unique_id("bad-timeout"),
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
}).await;

    assert!(result.is_err(), "register_job must reject timeout_sec = 0");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("timeout_sec") || err.contains("1"),
        "error message must reference the timeout constraint, got: {err}"
    );
}

// ── Client rejects null byte in spawn argument ────────────────────────────────

/// `register_job` must reject any `args` entry containing a null byte.
/// Null bytes cannot appear in OS argument vectors and would silently
/// truncate the argument or cause a panic at the OS level.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_register_job_rejects_null_byte_in_arg() {
    let nats = connect().await;
    let client = CronClient::new(nats).await.unwrap();

    let result = client.register_job(&JobConfig {
        id: unique_id("bad-null-byte"),
        schedule: Schedule::Interval { interval_sec: 60 },
        action: Action::Spawn {
            bin: "/usr/bin/true".to_string(),
            args: vec!["valid".to_string(), "bad\0arg".to_string()],
            concurrent: false,
            timeout_sec: None,
        },
        enabled: true,
        payload: None,
        retry: None,
}).await;

    assert!(result.is_err(), "register_job must reject args containing a null byte");
    let err = result.unwrap_err().to_string();
    assert!(
        err.to_lowercase().contains("null"),
        "error message must mention null byte, got: {err}"
    );
}

// ── Hot-reload from Publish → Spawn ──────────────────────────────────────────

/// Reload a running job from `Action::Publish` to `Action::Spawn`.
/// After the reload the NATS subject must go silent and the spawned binary
/// must execute — the complement of `test_action_change_spawn_to_publish`.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_action_change_publish_to_spawn() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-pub-to-spawn");
    let subject = format!("cron.pub-to-spawn.{id}");
    let sentinel: PathBuf = std::env::temp_dir().join(format!("trogon-pub-to-spawn-{id}"));
    let _ = std::fs::remove_file(&sentinel);

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    // Start with Publish action.
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Confirm publish fires.
    tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await
        .expect("Timed out waiting for initial publish tick")
        .unwrap()
        .unwrap();

    // Hot-reload to Spawn.
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/usr/bin/touch".to_string(),
            args: vec![sentinel.to_str().unwrap().to_string()],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    // Spawn must run within 3 s.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        if sentinel.exists() { break; }
        assert!(tokio::time::Instant::now() < deadline,
            "Spawn did not fire after Publish->Spawn hot-reload");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Old publish subject must be silent.
    let stale = tokio::time::timeout(Duration::from_millis(500), sub.next()).await;
    assert!(stale.is_err(), "Publish subject must be silent after switching to Spawn action");

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&sentinel);
}

// ── Spawn fire rate matches interval_sec ─────────────────────────────────────

/// Count how many times a fast-completing `concurrent: false` spawn job fires
/// over a 5-second window with `interval_sec = 1`.
///
/// A single leader fires ~5 times.  Significantly more means double-firing;
/// significantly fewer means the scheduler is skipping ticks.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_fire_rate_matches_interval() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-spawn-rate");
    let counter: PathBuf = std::env::temp_dir().join(format!("trogon-spawn-rate-{id}"));
    let _ = std::fs::remove_file(&counter);

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(),
                format!("echo x >> '{}'", counter.to_str().unwrap())],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    tokio::time::sleep(Duration::from_secs(5)).await;

    let count = std::fs::read_to_string(&counter)
        .unwrap_or_default().lines().count();

    // ~5 expected; generous CI bounds.
    assert!(count >= 3, "spawn fired too infrequently in 5 s: {count} times");
    assert!(count <= 8, "spawn fired too frequently in 5 s (double-firing?): {count} times");

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&counter);
}

// ── CRON_PAYLOAD is absent when payload is None ───────────────────────────────

/// When `JobConfig.payload` is `None` the spawned process must NOT have
/// `CRON_PAYLOAD` set in its environment.  The script uses the shell default
/// `${CRON_PAYLOAD:-NOT_SET}` to distinguish "unset" from "empty string".
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_no_cron_payload_when_payload_none() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-no-payload-env");
    let output: PathBuf = std::env::temp_dir().join(format!("trogon-no-payload-{id}"));
    let _ = std::fs::remove_file(&output);

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                format!("printf '%s' \"${{CRON_PAYLOAD:-NOT_SET}}\" > '{}'",
                    output.to_str().unwrap()),
            ],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: None,   // <── no payload
        retry: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if output.exists() {
            let content = std::fs::read_to_string(&output).unwrap();
            if !content.is_empty() { break; }
        }
        assert!(tokio::time::Instant::now() < deadline, "Spawn never wrote output");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let written = std::fs::read_to_string(&output).unwrap();
    assert_eq!(
        written.trim(), "NOT_SET",
        "CRON_PAYLOAD must not be set in env when payload is None"
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&output);
}

// ── Job removed before scheduler starts is not loaded ────────────────────────

/// Register a job then immediately delete it — all before the scheduler
/// starts.  The scheduler must load an empty state and fire nothing.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_register_remove_before_scheduler_no_fire() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-prereg-rm");
    let subject = format!("cron.prereg-rm.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    // Register then immediately remove — before the scheduler is started.
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();
    client.remove_job(&id).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // No tick must arrive: the job was deleted before the scheduler loaded.
    let result = tokio::time::timeout(Duration::from_secs(3), sub.next()).await;
    assert!(result.is_err(), "Deleted-before-start job must not fire");

    handle.abort();
}

// ── "* * * * * *" cron expression fires every second ─────────────────────────

/// A cron expression with all wildcards (`* * * * * *`) fires once per second.
/// Collect three consecutive ticks and verify each arrives within 3 s.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_cron_every_second_expression() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-cron-every-sec");
    let subject = format!("cron.every-sec.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Cron { expr: "* * * * * *".to_string() },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    for i in 0..3 {
        let msg = tokio::time::timeout(Duration::from_secs(3), sub.next())
            .await
            .unwrap_or_else(|_| panic!("Timed out on tick #{}", i + 1))
            .unwrap()
            .unwrap();
        let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
        assert_eq!(tick.job_id, id, "job_id mismatch on tick #{}", i + 1);
    }

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Disabling a job mid-run: current process completes, no new fires ──────────

/// Start a `concurrent: false` spawn job that sleeps 3 s.
/// After the first invocation begins, disable the job via hot-reload.
/// The running process must be allowed to complete.
/// After completion, no new invocations must start because the job is disabled.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_disable_during_active_spawn_no_new_fires() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-disable-midrun");
    let counter: PathBuf = std::env::temp_dir().join(format!("trogon-disable-midrun-{id}"));
    let _ = std::fs::remove_file(&counter);

    // Each invocation appends a line then sleeps 3 s.
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(),
                format!("echo x >> '{}'; sleep 3", counter.to_str().unwrap())],
            concurrent: false,
            timeout_sec: Some(10),
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Wait for first invocation to start.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if counter.exists() { break; }
        assert!(tokio::time::Instant::now() < deadline, "First invocation never started");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Disable while the process is still sleeping.
    client.set_enabled(&id, false).await.unwrap();

    // Wait long enough for the first process to finish (3 s sleep) plus
    // another 2 s for any would-be second invocation to start.
    tokio::time::sleep(Duration::from_secs(5)).await;

    let count = std::fs::read_to_string(&counter)
        .unwrap_or_default().lines().count();
    assert_eq!(
        count, 1,
        "exactly 1 invocation must have run; disabling mid-run must prevent new fires (got {count})"
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&counter);
}

// ── Long-interval job fires exactly once in a short window ───────────────────

/// An interval job with `interval_sec = 30` fires immediately on load but
/// must not fire a second time within a 3-second observation window.
/// Proves the "fires once, then waits for the full interval" semantics.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_long_interval_fires_exactly_once() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-long-interval");
    let subject = format!("cron.long-interval.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 30 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // First tick must arrive quickly (immediate-fire semantics).
    let msg = tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await
        .expect("Timed out — interval job did not fire immediately on load")
        .unwrap()
        .unwrap();
    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(tick.job_id, id);

    // No second tick must arrive in the next 3 s (next fire is in ~30 s).
    let second = tokio::time::timeout(Duration::from_secs(3), sub.next()).await;
    assert!(
        second.is_err(),
        "interval_sec=30 job must not fire twice within 3 s of the first tick"
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Multi-level NATS subject (`cron.a.b.c`) is captured by the stream ─────────

/// The CRON_TICKS stream uses `cron.>` which matches any number of dot-separated
/// levels.  A subject like `cron.backup.db.daily.<id>` must be published and
/// received correctly.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_cron_subject_multi_level() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-multi-level");
    let subject = format!("cron.backup.db.daily.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let msg = tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await
        .expect("Timed out — multi-level subject tick never arrived")
        .unwrap()
        .unwrap();
    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(tick.job_id, id, "job_id must match for multi-level subject");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── KV purge operation removes job from the running scheduler ─────────────────

/// `Operation::Purge` in `handle_config_change` is handled the same as
/// `Operation::Delete`.  Use the raw NATS KV API to purge the job key and
/// verify the scheduler stops firing ticks.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_kv_purge_removes_job() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-kv-purge");
    let subject = format!("cron.kv-purge.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Confirm the job is firing.
    tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await
        .expect("Job must fire before purge")
        .unwrap()
        .unwrap();

    // Purge the KV key directly — distinct from client.remove_job (which calls delete).
    let kv = js.get_key_value("cron_configs").await.unwrap();
    kv.purge(format!("jobs.{id}")).await.unwrap();

    // After purge the scheduler must stop firing.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let stale = tokio::time::timeout(Duration::from_secs(2), sub.next()).await;
    assert!(stale.is_err(), "Job must be silent after KV purge");

    handle.abort();
}

// ── Leader-lock TTL is renewed: scheduler fires past the 10-second TTL ────────

/// The leader KV bucket has `max_age = 10 s`.  If the running scheduler did
/// not actively renew the lock, it would lose leadership after 10 s and stop
/// firing.  This test runs for 13 s and counts ticks — if the count is high
/// enough the lock was successfully renewed throughout.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_leader_lock_ttl_renewal() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-ttl-renewal");
    let subject = format!("cron.ttl-renewal.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Collect ticks over 13 s — past the 10-second leader TTL.
    let mut count = 0usize;
    let window = tokio::time::sleep(Duration::from_secs(13));
    tokio::pin!(window);
    loop {
        tokio::select! {
            biased;
            _ = &mut window => break,
            msg = sub.next() => { if msg.is_some() { count += 1; } }
        }
    }

    // At interval=1s over 13s a single renewed leader fires ~13 ticks.
    // If the lock expired at 10s and was not renewed, ticks would stop → count ≤ ~10.
    // We assert ≥ 10 to confirm firing continued past the TTL boundary.
    assert!(
        count >= 10,
        "scheduler must keep firing past the 10-second leader TTL (got {count} ticks in 13 s)"
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Non-executable binary is skipped gracefully ───────────────────────────────

/// Register a spawn job whose `bin` points to a regular file that exists but
/// has no execute permission.  The scheduler must log the error and skip the
/// job — without crashing — and other jobs must continue running normally.
///
/// This exercises the `mode() & 0o111 == 0` branch in `validate_spawn_config`,
/// which is distinct from the nonexistent-bin test.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
#[cfg(unix)]
async fn test_non_executable_bin_is_skipped() {
    use std::os::unix::fs::PermissionsExt;

    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();

    // Create a regular (non-executable) file.
    let non_exec: PathBuf = std::env::temp_dir().join(format!("trogon-non-exec-{}", unique_id("f")));
    std::fs::write(&non_exec, b"not a script").unwrap();
    std::fs::set_permissions(&non_exec, std::fs::Permissions::from_mode(0o644)).unwrap();

    let bad_id = unique_id("test-non-exec-bad");
    let good_id = unique_id("test-non-exec-good");
    let subject = format!("cron.non-exec-good.{good_id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    // Register the bad (non-executable) spawn job.
    client.register_job(&JobConfig {
        id: bad_id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: non_exec.to_str().unwrap().to_string(),
            args: vec![],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap(); // client accepts it (no exec check on client side)

    // Register a good publish job alongside it.
    client.register_job(&JobConfig {
        id: good_id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Good job must still fire — scheduler must not crash on the bad job.
    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("Good job must fire even when a non-executable spawn job is registered")
        .unwrap()
        .unwrap();
    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(tick.job_id, good_id);

    handle.abort();
    client.remove_job(&bad_id).await.unwrap();
    client.remove_job(&good_id).await.unwrap();
    let _ = std::fs::remove_file(&non_exec);
}

// ── Twenty jobs all fire ──────────────────────────────────────────────────────

/// Scale test: register 20 interval jobs simultaneously and verify every one
/// fires at least once within 5 seconds.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_twenty_jobs_all_fire() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();

    const N: usize = 20;
    let ids: Vec<String> = (0..N).map(|i| unique_id(&format!("test-20jobs-{i}"))).collect();
    let subjects: Vec<String> = ids.iter().map(|id| format!("cron.twentyjobs.{id}")).collect();

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
        retry: None,
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

// ── Invalid cron expression is skipped gracefully by the scheduler ────────────

/// `CronClient::register_job` does not validate cron expressions, so a
/// syntactically invalid expression can reach the scheduler.  When
/// `build_job_state` fails to parse it the scheduler must log the error and
/// skip that job — without crashing — while other jobs keep firing normally.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_invalid_cron_expression_skipped_by_scheduler() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();

    let bad_id  = unique_id("test-bad-cron");
    let good_id = unique_id("test-good-cron");
    let subject = format!("cron.good-cron.{good_id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    // Register a job whose cron expression is unparseable — client accepts it.
    client.register_job(&JobConfig {
        id: bad_id.clone(),
        schedule: Schedule::Cron { expr: "not-a-valid-cron-expression".to_string() },
        action: Action::Publish { subject: format!("cron.bad-cron.{bad_id}") },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    // Register a valid job alongside the bad one.
    client.register_job(&JobConfig {
        id: good_id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // The good job must fire — bad cron expression must not crash the scheduler.
    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("Good job must fire even when a bad cron expression is registered")
        .unwrap()
        .unwrap();
    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(tick.job_id, good_id);

    handle.abort();
    client.remove_job(&bad_id).await.unwrap();
    client.remove_job(&good_id).await.unwrap();
}

// ── Payload as a JSON array round-trips correctly ─────────────────────────────

/// Verify that a `payload` of type JSON array (`[1, "two", true, null]`) —
/// not an object — survives the full round-trip: KV storage → scheduler tick
/// → NATS message → deserialized `TickPayload`.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_payload_array_type() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-payload-array");
    let subject = format!("cron.payload-array.{id}");

    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    let array_payload = serde_json::json!([1, "two", true, null]);

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: Some(array_payload.clone()),
        retry: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let msg = tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await
        .expect("Timed out waiting for tick with array payload")
        .unwrap()
        .unwrap();
    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(
        tick.payload.unwrap(), array_payload,
        "JSON array payload must survive the round-trip unchanged"
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── CRON_FIRED_AT env var is a valid RFC3339 timestamp ───────────────────────

/// The spawned process receives `CRON_FIRED_AT` as a string.  Write it to a
/// file and parse it with `chrono` to confirm it follows RFC3339 format.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_cron_fired_at_env_var_is_rfc3339() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-fired-at-fmt");
    let output: PathBuf = std::env::temp_dir().join(format!("trogon-fired-at-{id}"));
    let _ = std::fs::remove_file(&output);

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                format!("printf '%s' \"$CRON_FIRED_AT\" > '{}'", output.to_str().unwrap()),
            ],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if output.exists() {
            let s = std::fs::read_to_string(&output).unwrap();
            if !s.is_empty() { break; }
        }
        assert!(tokio::time::Instant::now() < deadline, "Spawn never wrote output");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let raw = std::fs::read_to_string(&output).unwrap();
    chrono::DateTime::parse_from_rfc3339(raw.trim())
        .unwrap_or_else(|e| panic!("CRON_FIRED_AT '{}' is not valid RFC3339: {e}", raw.trim()));

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&output);
}

// ── Each job's tick carries that job's own payload ────────────────────────────

/// Register two jobs with different payloads.  Collect one tick from each and
/// verify the payload in each tick matches that specific job's payload —
/// ticks must not mix up payloads across jobs.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_two_jobs_each_carry_own_payload() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();

    let id_a = unique_id("test-own-payload-a");
    let id_b = unique_id("test-own-payload-b");
    let sub_a_str = format!("cron.own-payload-a.{id_a}");
    let sub_b_str = format!("cron.own-payload-b.{id_b}");

    let mut sub_a = cron_messages(&nats, &js, sub_a_str.clone()).await;
    let mut sub_b = cron_messages(&nats, &js, sub_b_str.clone()).await;

    let payload_a = serde_json::json!({ "job": "alpha", "v": 1 });
    let payload_b = serde_json::json!({ "job": "beta",  "v": 2 });

    client.register_job(&JobConfig {
        id: id_a.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: sub_a_str.clone() },
        enabled: true,
        payload: Some(payload_a.clone()),
        retry: None,
    }).await.unwrap();

    client.register_job(&JobConfig {
        id: id_b.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: sub_b_str.clone() },
        enabled: true,
        payload: Some(payload_b.clone()),
        retry: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let msg_a = tokio::time::timeout(Duration::from_secs(3), sub_a.next())
        .await.expect("Timed out on job A tick").unwrap().unwrap();
    let msg_b = tokio::time::timeout(Duration::from_secs(3), sub_b.next())
        .await.expect("Timed out on job B tick").unwrap().unwrap();

    let tick_a: trogon_cron::TickPayload = serde_json::from_slice(&msg_a.payload).unwrap();
    let tick_b: trogon_cron::TickPayload = serde_json::from_slice(&msg_b.payload).unwrap();

    assert_eq!(tick_a.job_id, id_a, "tick A must carry job A's id");
    assert_eq!(tick_b.job_id, id_b, "tick B must carry job B's id");
    assert_eq!(tick_a.payload.unwrap(), payload_a, "tick A must carry job A's payload");
    assert_eq!(tick_b.payload.unwrap(), payload_b, "tick B must carry job B's payload");

    handle.abort();
    client.remove_job(&id_a).await.unwrap();
    client.remove_job(&id_b).await.unwrap();
}

// ── concurrent:false rate-limits a slow process ───────────────────────────────

/// With `concurrent: false` and a process that sleeps 2 s — longer than the
/// 1-second interval — each tick that fires while the previous run is still
/// active must be skipped.
///
/// Over an 8-second window we expect ~4 fires (one every ~2 s), not ~8.
/// This quantifies the rate-limiting effect of the concurrency guard.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_concurrent_false_rate_limits_slow_process() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-conc-false-rate");
    let counter: PathBuf = std::env::temp_dir().join(format!("trogon-conc-rate-{id}"));
    let _ = std::fs::remove_file(&counter);

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(),
                format!("echo x >> '{}'; sleep 2", counter.to_str().unwrap())],
            concurrent: false,
            timeout_sec: Some(10),
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    tokio::time::sleep(Duration::from_secs(8)).await;

    let count = std::fs::read_to_string(&counter)
        .unwrap_or_default().lines().count();

    // 2-second sleep + 1-second interval → ~1 fire per 2 s → ~4 fires in 8 s.
    // Without skipping we would see ~8.  Generous CI bounds: 2–6.
    assert!(count >= 2, "too few fires for a 2-s process over 8 s: {count}");
    assert!(count <= 6, "too many fires — concurrent:false skipping may not be working: {count}");

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&counter);
}

// ── concurrent:true does NOT rate-limit a slow process ───────────────────────

/// With `concurrent: true` a new invocation can start even if the previous
/// one is still running.  Use a 2-second sleeping process with a 1-second
/// interval: over 8 s we expect ≥5 fires — roughly one per interval tick
/// regardless of process duration.  This is the inverse of
/// `test_concurrent_false_rate_limits_slow_process`.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_concurrent_true_not_rate_limited() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-conc-true-rate");
    let counter: PathBuf = std::env::temp_dir().join(format!("trogon-conc-true-{id}"));
    let _ = std::fs::remove_file(&counter);

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(),
                format!("echo x >> '{}'; sleep 2", counter.to_str().unwrap())],
            concurrent: true,
            timeout_sec: Some(15),
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    tokio::time::sleep(Duration::from_secs(8)).await;

    let count = std::fs::read_to_string(&counter)
        .unwrap_or_default().lines().count();

    // concurrent:true — new spawn every ~1 s regardless of the 2-s sleep.
    // Expect at least 5 fires in 8 s (generous lower bound for CI).
    assert!(count >= 5, "concurrent:true should not rate-limit; got only {count} fires in 8 s");

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&counter);
}

// ── Cron */3 expression fires every 3 seconds ─────────────────────────────────

/// A cron expression `*/3 * * * * *` (every 3rd second) must produce ticks
/// roughly 3 seconds apart.  Collect two consecutive ticks and check the
/// wall-clock gap between their `fired_at` timestamps is between 2 and 5 s.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_cron_every_3_seconds_timing() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-cron-3s");
    let subject = format!("cron.cron3s.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Cron { expr: "*/3 * * * * *".to_string() },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let msg1 = tokio::time::timeout(Duration::from_secs(8), sub.next())
        .await.expect("Timed out waiting for first tick").unwrap().unwrap();
    let msg2 = tokio::time::timeout(Duration::from_secs(8), sub.next())
        .await.expect("Timed out waiting for second tick").unwrap().unwrap();

    let tick1: trogon_cron::TickPayload = serde_json::from_slice(&msg1.payload).unwrap();
    let tick2: trogon_cron::TickPayload = serde_json::from_slice(&msg2.payload).unwrap();

    let t1 = tick1.fired_at;
    let t2 = tick2.fired_at;
    let gap_secs = (t2 - t1).num_milliseconds().abs() as f64 / 1000.0;

    assert!(
        gap_secs >= 2.0 && gap_secs <= 5.0,
        "*/3 cron should fire ~3 s apart, got {gap_secs:.2} s"
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Each spawn job writes its own correct CRON_JOB_ID ────────────────────────

/// Register two spawn jobs.  Each writes `$CRON_JOB_ID` to a separate temp
/// file.  After both fire, read each file and confirm it contains exactly the
/// ID configured for that job — no cross-contamination.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_two_spawn_jobs_correct_job_ids() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();

    let id_a = unique_id("test-jobid-a");
    let id_b = unique_id("test-jobid-b");
    let out_a: PathBuf = std::env::temp_dir().join(format!("trogon-jobid-a-{id_a}"));
    let out_b: PathBuf = std::env::temp_dir().join(format!("trogon-jobid-b-{id_b}"));
    let _ = std::fs::remove_file(&out_a);
    let _ = std::fs::remove_file(&out_b);

    for (id, out) in [(&id_a, &out_a), (&id_b, &out_b)] {
        client.register_job(&JobConfig {
            id: id.clone(),
            schedule: Schedule::Interval { interval_sec: 1 },
            action: Action::Spawn {
                bin: "/bin/sh".to_string(),
                args: vec!["-c".to_string(),
                    format!("printf '%s' \"$CRON_JOB_ID\" > '{}'", out.to_str().unwrap())],
                concurrent: false,
                timeout_sec: Some(5),
            },
            enabled: true,
            payload: None,
        retry: None,
}).await.unwrap();
    }

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    loop {
        if out_a.exists() && out_b.exists() {
            let ca = std::fs::read_to_string(&out_a).unwrap_or_default();
            let cb = std::fs::read_to_string(&out_b).unwrap_or_default();
            if !ca.is_empty() && !cb.is_empty() { break; }
        }
        assert!(tokio::time::Instant::now() < deadline, "Not both spawn jobs wrote their output in time");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let written_a = std::fs::read_to_string(&out_a).unwrap();
    let written_b = std::fs::read_to_string(&out_b).unwrap();

    assert_eq!(written_a.trim(), id_a, "Job A wrote wrong CRON_JOB_ID");
    assert_eq!(written_b.trim(), id_b, "Job B wrote wrong CRON_JOB_ID");

    handle.abort();
    client.remove_job(&id_a).await.unwrap();
    client.remove_job(&id_b).await.unwrap();
    let _ = std::fs::remove_file(&out_a);
    let _ = std::fs::remove_file(&out_b);
}

// ── fired_at timestamps are monotonically increasing ─────────────────────────

/// Collect 5 consecutive ticks from an interval job and assert that each
/// tick's `fired_at` timestamp is strictly after the previous one.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_interval_tick_timestamps_monotonic() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-ts-monotonic");
    let subject = format!("cron.ts-monotonic.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let mut timestamps = Vec::new();
    for i in 0..5usize {
        let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
            .await
            .unwrap_or_else(|_| panic!("Timed out waiting for tick #{i}"))
            .unwrap().unwrap();
        let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
        timestamps.push(tick.fired_at);
    }

    for i in 1..timestamps.len() {
        assert!(
            timestamps[i] > timestamps[i - 1],
            "fired_at not monotonically increasing: tick[{i}]={} <= tick[{}]={}",
            timestamps[i], i - 1, timestamps[i - 1]
        );
    }

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── All TickPayload fields are present and correct in a single tick ───────────

/// Fire exactly one tick and verify all four `TickPayload` fields:
/// - `job_id` matches the registered job id
/// - `fired_at` is a non-empty, valid RFC3339 string
/// - `execution_id` is a non-empty UUID-like string
/// - `payload` matches the registered payload value
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_tick_payload_all_fields_present() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-all-fields");
    let subject = format!("cron.all-fields.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    let expected_payload = serde_json::json!({ "check": "all-fields", "n": 42 });

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: Some(expected_payload.clone()),
        retry: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let msg = tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await.expect("Timed out waiting for tick").unwrap().unwrap();

    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();

    // job_id
    assert_eq!(tick.job_id, id, "job_id mismatch");

    // fired_at — must be a recent timestamp (within last 10 s)
    let age = chrono::Utc::now().signed_duration_since(tick.fired_at);
    assert!(
        age.num_seconds().abs() < 10,
        "fired_at is not recent: {:?}", tick.fired_at
    );

    // execution_id — non-empty
    assert!(!tick.execution_id.is_empty(), "execution_id must not be empty");

    // payload
    assert_eq!(
        tick.payload.as_ref().unwrap(), &expected_payload,
        "payload mismatch"
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Disabled job registered before scheduler start never fires ────────────────

/// A job with `enabled: false` that is already in the KV store when the
/// scheduler starts must never produce a tick — the scheduler must respect
/// the disabled flag from the initial snapshot.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_disabled_job_never_fires() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-disabled-start");
    let subject = format!("cron.disabled-start.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: false,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let result = tokio::time::timeout(Duration::from_secs(3), sub.next()).await;
    assert!(result.is_err(), "Disabled job must not fire at scheduler start");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Enabling a disabled job while scheduler runs starts ticks ─────────────────

/// Register a disabled job, start the scheduler (verify silence), then call
/// `set_enabled(true)` and confirm ticks begin arriving within 3 seconds.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_enable_disabled_job_starts_firing() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-enable-disabled");
    let subject = format!("cron.enable-disabled.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: false,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Wait 1 s to confirm job is silent while disabled.
    let silent = tokio::time::timeout(Duration::from_secs(1), sub.next()).await;
    assert!(silent.is_err(), "Disabled job must not fire before being enabled");

    // Now enable it — should trigger a hot-reload and start firing.
    client.set_enabled(&id, true).await.unwrap();

    tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await
        .expect("Job must fire after being enabled")
        .unwrap()
        .unwrap();

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Payload updated via hot-reload appears in subsequent ticks ─────────────────

/// Register a job with payload A, receive one tick (payload = A), then
/// hot-reload the job with payload B.  The next tick must carry payload B.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_payload_hot_reload_updates_tick() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-payload-reload");
    let subject = format!("cron.payload-reload.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    let payload_a = serde_json::json!({ "version": "A" });
    let payload_b = serde_json::json!({ "version": "B" });

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: Some(payload_a.clone()),
        retry: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Collect the first tick — must carry payload A.
    let msg_a = tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await.expect("Timed out waiting for first tick").unwrap().unwrap();
    let tick_a: trogon_cron::TickPayload = serde_json::from_slice(&msg_a.payload).unwrap();
    assert_eq!(tick_a.payload.as_ref().unwrap(), &payload_a, "First tick must carry payload A");

    // Hot-reload with payload B.
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: Some(payload_b.clone()),
        retry: None,
    }).await.unwrap();

    // Drain any already-queued ticks that may still carry payload A,
    // then assert the first tick with payload B arrives within 5 s.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "Timed out waiting for tick with payload B after hot-reload");
        let msg = tokio::time::timeout(remaining, sub.next())
            .await.expect("Timeout waiting for tick with B").unwrap().unwrap();
        let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
        if tick.payload.as_ref() == Some(&payload_b) {
            break; // Got the updated payload — test passes.
        }
    }

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── execution_id has UUID format ──────────────────────────────────────────────

/// The `execution_id` field of every tick must look like a UUID:
/// 36 characters, lowercase hex groups separated by exactly 4 hyphens
/// (e.g. `550e8400-e29b-41d4-a716-446655440000`).
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_execution_id_is_uuid_format() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-exec-uuid");
    let subject = format!("cron.exec-uuid.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let msg = tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await.expect("Timed out").unwrap().unwrap();
    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();

    let eid = &tick.execution_id;
    assert_eq!(eid.len(), 36, "execution_id must be 36 chars, got {eid:?}");
    assert_eq!(
        eid.chars().filter(|c| *c == '-').count(), 4,
        "execution_id must have 4 hyphens, got {eid:?}"
    );
    // Verify each segment is hex digits.
    let parts: Vec<&str> = eid.split('-').collect();
    for part in &parts {
        assert!(
            part.chars().all(|c| c.is_ascii_hexdigit()),
            "execution_id segment {part:?} contains non-hex chars"
        );
    }

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Extra positional args are forwarded to the spawned process ────────────────

/// When `args` contains positional arguments beyond `-c <script>`, they must
/// be available inside the shell as `$1`, `$2`, `$3`.
///
/// Uses the sh convention: `sh -c 'script' sh arg1 arg2 arg3`
/// where the second `sh` becomes `$0` and the rest become `$1`…`$3`.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_positional_args_forwarded() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-pos-args");
    let out: PathBuf = std::env::temp_dir().join(format!("trogon-posargs-{id}"));
    let _ = std::fs::remove_file(&out);

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                // Write $1 $2 $3 to a file, one per line.
                format!("printf '%s\\n' \"$1\" \"$2\" \"$3\" > '{}'", out.to_str().unwrap()),
                "sh".to_string(),    // $0 (argv[0] for the script)
                "alpha".to_string(), // $1
                "beta".to_string(),  // $2
                "gamma".to_string(), // $3
            ],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if out.exists() {
            let content = std::fs::read_to_string(&out).unwrap_or_default();
            if !content.trim().is_empty() { break; }
        }
        assert!(tokio::time::Instant::now() < deadline, "Spawn never wrote positional args output");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let written = std::fs::read_to_string(&out).unwrap();
    let lines: Vec<&str> = written.lines().collect();
    assert_eq!(lines.get(0).copied(), Some("alpha"), "$1 must be 'alpha'");
    assert_eq!(lines.get(1).copied(), Some("beta"),  "$2 must be 'beta'");
    assert_eq!(lines.get(2).copied(), Some("gamma"), "$3 must be 'gamma'");

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&out);
}

// ── Interval job fires immediately on scheduler load ──────────────────────────

/// `Schedule::Interval` must fire on the very first scheduler tick after
/// loading, without waiting the full `interval_sec`.  Even with a 30-second
/// interval the first tick must arrive within 2 seconds.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_interval_fires_immediately_on_load() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-imm-fire");
    let subject = format!("cron.imm-fire.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 30 }, // long interval
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Must fire well before the 30-second interval expires.
    tokio::time::timeout(Duration::from_secs(2), sub.next())
        .await
        .expect("Interval job must fire immediately on load, not after interval_sec")
        .unwrap()
        .unwrap();

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── CRON_EXECUTION_ID env var in spawned process has UUID format ──────────────

/// The spawned process receives `CRON_EXECUTION_ID` as an environment
/// variable.  Write it to a file and verify the value is a valid UUID
/// (36 chars, 4 hyphens, all hex segments).
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_cron_execution_id_env_var() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-exec-id-env");
    let out: PathBuf = std::env::temp_dir().join(format!("trogon-execid-{id}"));
    let _ = std::fs::remove_file(&out);

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                format!("printf '%s' \"$CRON_EXECUTION_ID\" > '{}'", out.to_str().unwrap()),
            ],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if out.exists() {
            let s = std::fs::read_to_string(&out).unwrap_or_default();
            if !s.is_empty() { break; }
        }
        assert!(tokio::time::Instant::now() < deadline, "Spawn never wrote CRON_EXECUTION_ID");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let eid = std::fs::read_to_string(&out).unwrap();
    let eid = eid.trim();
    assert_eq!(eid.len(), 36, "CRON_EXECUTION_ID must be 36 chars, got {eid:?}");
    assert_eq!(
        eid.chars().filter(|c| *c == '-').count(), 4,
        "CRON_EXECUTION_ID must have 4 hyphens, got {eid:?}"
    );
    for part in eid.split('-') {
        assert!(
            part.chars().all(|c| c.is_ascii_hexdigit()),
            "CRON_EXECUTION_ID segment {part:?} is not hex"
        );
    }

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&out);
}

// ── Two CronClient instances: last write wins ─────────────────────────────────

/// Two independent `CronClient` instances register the same job ID with
/// different publish subjects.  The second registration must overwrite the
/// first in the KV store, and the scheduler must fire on the second subject
/// only.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_two_clients_last_write_wins() {
    let (nats, js) = connect_js().await;
    let client1 = CronClient::new(nats.clone()).await.unwrap();
    let client2 = CronClient::new(nats.clone()).await.unwrap();

    let id = unique_id("test-two-clients");
    let subject_a = format!("cron.two-clients-a.{id}");
    let subject_b = format!("cron.two-clients-b.{id}");

    let mut sub_a = cron_messages(&nats, &js, subject_a.clone()).await;
    let mut sub_b = cron_messages(&nats, &js, subject_b.clone()).await;

    // client1 writes first…
    client1.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject_a.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    // …client2 overwrites with a different subject.
    client2.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject_b.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // subject_b must fire (last write wins).
    tokio::time::timeout(Duration::from_secs(3), sub_b.next())
        .await
        .expect("subject_b must fire — last write should win")
        .unwrap()
        .unwrap();

    // subject_a must be silent.
    let silent = tokio::time::timeout(Duration::from_millis(500), sub_a.next()).await;
    assert!(silent.is_err(), "subject_a must not fire after being overwritten");

    handle.abort();
    client1.remove_job(&id).await.unwrap();
}

// ── Removing a non-existent job is a no-op ────────────────────────────────────

/// `CronClient::remove_job` must return `Ok(())` even when the given job ID
/// has never been registered.  This validates the documented "no-op if not
/// found" contract.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_remove_nonexistent_job_no_error() {
    let (nats, _js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();

    // ID that has never been registered.
    let id = unique_id("test-remove-nonexistent");
    let result = client.remove_job(&id).await;
    assert!(result.is_ok(), "remove_job on unknown ID must return Ok, got: {result:?}");
}

// ── concurrent:false respawns after previous process exits normally ────────────

/// With `concurrent: false`, once a spawned process exits the `is_running`
/// flag must clear so the next tick can launch a new process.
///
/// Use a process that exits quickly (sleep 0).  Over 4 s the job must fire
/// at least twice, proving the flag is properly reset after each completion.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_concurrent_false_respawns_after_exit() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-conc-respawn");
    let counter: PathBuf = std::env::temp_dir().join(format!("trogon-respawn-{id}"));
    let _ = std::fs::remove_file(&counter);

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(),
                format!("echo x >> '{}'", counter.to_str().unwrap())],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    tokio::time::sleep(Duration::from_secs(4)).await;

    let count = std::fs::read_to_string(&counter)
        .unwrap_or_default().lines().count();

    // Fast process + 1-s interval → should fire at least twice in 4 s.
    assert!(
        count >= 2,
        "concurrent:false must respawn after exit; only {count} fires in 4 s"
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&counter);
}

// ── get_job returns None for an unknown id ────────────────────────────────────

/// `CronClient::get_job` must return `Ok(None)` for a job id that was never
/// registered, confirming the documented contract.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_get_job_returns_none_for_unknown_id() {
    let (nats, _js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();

    let result = client.get_job(&unique_id("test-get-none")).await.unwrap();
    assert!(result.is_none(), "get_job must return None for an unregistered id");
}

// ── Spawn with non-zero exit code: scheduler continues firing ─────────────────

/// A spawned process that exits with a non-zero status code must not crash
/// or freeze the scheduler.  The `is_running` flag must clear on exit
/// regardless of exit code so subsequent ticks can spawn again.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_nonzero_exit_continues_firing() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-nonzero-exit");
    let counter: PathBuf = std::env::temp_dir().join(format!("trogon-nonzero-{id}"));
    let _ = std::fs::remove_file(&counter);

    // Script appends a line then exits with code 1.
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(),
                format!("echo x >> '{}'; exit 1", counter.to_str().unwrap())],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    tokio::time::sleep(Duration::from_secs(4)).await;

    let count = std::fs::read_to_string(&counter)
        .unwrap_or_default().lines().count();

    // Non-zero exit must not freeze spawning: expect ≥2 fires in 4 s.
    assert!(
        count >= 2,
        "Scheduler must continue spawning after non-zero exit; got {count} fires"
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&counter);
}

// ── Large JSON payload survives the round-trip ────────────────────────────────

/// A deeply nested JSON payload (~1 KB) must survive the full path:
/// KV storage → scheduler tick → NATS message → deserialized `TickPayload`.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_large_json_payload_round_trips() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-large-payload");
    let subject = format!("cron.large-payload.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    // Build a ~1 KB payload.
    let large = serde_json::json!({
        "level1": {
            "level2": {
                "level3": {
                    "data": "A".repeat(512),
                    "numbers": (0..32).collect::<Vec<_>>(),
                    "flag": true
                }
            }
        },
        "tags": ["alpha", "beta", "gamma", "delta", "epsilon"]
    });

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: Some(large.clone()),
        retry: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let msg = tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await.expect("Timed out waiting for large-payload tick").unwrap().unwrap();
    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();

    assert_eq!(
        tick.payload.unwrap(), large,
        "Large JSON payload must survive the round-trip unchanged"
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Scheduler with no pre-existing jobs picks up dynamically added job ─────────

/// Start the scheduler when the KV store has no job entries, then register a
/// job through the client.  The scheduler must pick it up via the KV watcher
/// and begin firing within 3 seconds.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_scheduler_picks_up_dynamically_added_job() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-dynamic-add");
    let subject = format!("cron.dynamic-add.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    reset_leader_lock(&js).await;

    // Start the scheduler BEFORE registering any job.
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Give scheduler a moment to settle, then add the job dynamically.
    tokio::time::sleep(Duration::from_millis(300)).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    // Scheduler must pick up the new job via the watcher and fire.
    tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await
        .expect("Scheduler must fire dynamically added job via KV watcher")
        .unwrap()
        .unwrap();

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── execution_id is unique across every tick ──────────────────────────────────

/// Collect 10 consecutive ticks and assert that every `execution_id` is
/// distinct — the scheduler must generate a fresh UUID for each invocation.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_execution_ids_never_repeat_across_ticks() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-exec-unique");
    let subject = format!("cron.exec-unique.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let mut seen = std::collections::HashSet::new();
    for i in 0..10usize {
        let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
            .await
            .unwrap_or_else(|_| panic!("Timed out waiting for tick #{i}"))
            .unwrap().unwrap();
        let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
        let eid = tick.execution_id.clone();
        assert!(
            seen.insert(eid.clone()),
            "execution_id {eid:?} repeated on tick #{i}"
        );
    }

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── register_job rejects empty publish subject ────────────────────────────────

/// An empty string as the publish subject must be rejected immediately by
/// `CronClient::register_job` with an `InvalidJobConfig` error because it
/// does not start with `"cron."`.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_register_job_rejects_empty_publish_subject() {
    let (nats, _js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();

    let result = client.register_job(&JobConfig {
        id: unique_id("test-empty-subj"),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: "".to_string() },
        enabled: true,
        payload: None,
        retry: None,
}).await;

    assert!(
        matches!(result, Err(trogon_cron::CronError::InvalidJobConfig { .. })),
        "Empty subject must return InvalidJobConfig, got: {result:?}"
    );
}

// ── list_jobs reflects exact add/remove sequence ──────────────────────────────

/// Register 3 jobs with unique IDs, remove the middle one, then call
/// `list_jobs()` and assert the returned list contains exactly the 2
/// remaining IDs and not the removed one.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_list_jobs_exact_contents_after_add_remove() {
    let (nats, _js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();

    let id_a = unique_id("test-list-exact-a");
    let id_b = unique_id("test-list-exact-b");
    let id_c = unique_id("test-list-exact-c");

    for id in [&id_a, &id_b, &id_c] {
        client.register_job(&JobConfig {
            id: id.clone(),
            schedule: Schedule::Interval { interval_sec: 60 },
            action: Action::Publish { subject: format!("cron.list-exact.{id}") },
            enabled: true,
            payload: None,
        retry: None,
}).await.unwrap();
    }

    // Remove the middle job.
    client.remove_job(&id_b).await.unwrap();

    let jobs = client.list_jobs().await.unwrap();
    let ids: Vec<&str> = jobs.iter().map(|j| j.id.as_str()).collect();

    assert!(ids.contains(&id_a.as_str()), "id_a must still be listed");
    assert!(ids.contains(&id_c.as_str()), "id_c must still be listed");
    assert!(!ids.contains(&id_b.as_str()), "id_b must not appear after removal");

    // Cleanup.
    client.remove_job(&id_a).await.unwrap();
    client.remove_job(&id_c).await.unwrap();
}

// ── JobConfig round-trips through the KV store unchanged ─────────────────────

/// Register a `JobConfig` with every field populated, retrieve it via
/// `get_job()`, and assert every field survives the serialise→store→
/// deserialise round-trip without alteration.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_job_config_round_trips_through_kv() {
    let (nats, _js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-kv-roundtrip");

    let original = JobConfig {
        id: id.clone(),
        schedule: Schedule::Cron { expr: "*/5 * * * * *".to_string() },
        action: Action::Publish { subject: format!("cron.kv-roundtrip.{id}") },
        enabled: false,
        payload: Some(serde_json::json!({ "round": "trip", "n": 99 })),
        retry: None,
};

    client.register_job(&original).await.unwrap();
    let retrieved = client.get_job(&id).await.unwrap()
        .expect("Job must be retrievable after registration");

    assert_eq!(retrieved.id, original.id);
    assert_eq!(retrieved.enabled, original.enabled);
    assert_eq!(retrieved.payload, original.payload);
    // Schedule variant preserved.
    assert!(
        matches!(retrieved.schedule, Schedule::Cron { ref expr } if expr == "*/5 * * * * *"),
        "Schedule must round-trip correctly, got: {:?}", retrieved.schedule
    );

    client.remove_job(&id).await.unwrap();
}

// ── Timeout longer than process duration does not prematurely kill ────────────

/// A spawned process that finishes in ~100 ms with a generous `timeout_sec`
/// of 10 s must not be killed early.  The `is_running` flag clears on natural
/// exit and the job fires at least 3 times in 4 seconds.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_timeout_does_not_prematurely_kill() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-timeout-safe");
    let counter: PathBuf = std::env::temp_dir().join(format!("trogon-timeout-safe-{id}"));
    let _ = std::fs::remove_file(&counter);

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(),
                format!("echo x >> '{}'; sleep 0.1", counter.to_str().unwrap())],
            concurrent: false,
            timeout_sec: Some(10), // much longer than the ~0.1 s process
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    tokio::time::sleep(Duration::from_secs(4)).await;

    let count = std::fs::read_to_string(&counter)
        .unwrap_or_default().lines().count();

    assert!(
        count >= 3,
        "Fast process with long timeout must fire ≥3 times in 4 s; got {count}"
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&counter);
}

// ── All ticks from the same job carry the same job_id ─────────────────────────

/// Collect 5 consecutive ticks and assert that every tick's `job_id` field
/// equals the registered job ID — no tick must carry a wrong or empty id.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_all_ticks_carry_same_job_id() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-same-jobid");
    let subject = format!("cron.same-jobid.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    for i in 0..5usize {
        let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
            .await
            .unwrap_or_else(|_| panic!("Timed out on tick #{i}"))
            .unwrap().unwrap();
        let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
        assert_eq!(
            tick.job_id, id,
            "Tick #{i} must carry job_id={id}, got {:?}", tick.job_id
        );
    }

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Publish subject with hyphens and multiple dots works end-to-end ───────────

/// A multi-segment subject containing hyphens (`cron.backup-daily.svc.{id}`)
/// must be accepted by the client, stored in KV, and delivered by the
/// scheduler — the NATS subject wildcard `cron.>` covers all sub-levels.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_publish_subject_with_hyphens_and_dots() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-hyphen-subj");
    let subject = format!("cron.backup-daily.my-service.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await
        .expect("Hyphenated multi-segment subject must fire")
        .unwrap()
        .unwrap();

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Spawn without timeout: process runs to natural completion ─────────────────

/// `timeout_sec: None` means no kill deadline is set.  A fast process must
/// complete normally and the `is_running` flag must clear, allowing the job
/// to fire at least 3 times in 4 seconds.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_no_timeout_fires_repeatedly() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-no-timeout");
    let counter: PathBuf = std::env::temp_dir().join(format!("trogon-no-timeout-{id}"));
    let _ = std::fs::remove_file(&counter);

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(),
                format!("echo x >> '{}'", counter.to_str().unwrap())],
            concurrent: false,
            timeout_sec: None,
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    tokio::time::sleep(Duration::from_secs(4)).await;

    let count = std::fs::read_to_string(&counter)
        .unwrap_or_default().lines().count();

    assert!(count >= 3, "No-timeout spawn must fire ≥3 times in 4 s; got {count}");

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&counter);
}

// ── Unicode payload round-trips through NATS unchanged ────────────────────────

/// A payload containing non-ASCII characters (accented letters, CJK,
/// emoji) must survive JSON serialisation → KV → scheduler → NATS message
/// → deserialisation without any corruption.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_unicode_payload_round_trips() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-unicode-payload");
    let subject = format!("cron.unicode-payload.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    let unicode = serde_json::json!({
        "latin":   "héllo wörld",
        "chinese": "你好世界",
        "emoji":   "🎉🚀✅",
        "arabic":  "مرحبا"
    });

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: Some(unicode.clone()),
        retry: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let msg = tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await.expect("Timed out").unwrap().unwrap();
    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();

    assert_eq!(tick.payload.unwrap(), unicode,
        "Unicode payload must survive the round-trip unchanged");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── get_job reflects set_enabled(false) immediately ──────────────────────────

/// After calling `set_enabled(false)` the KV store is updated synchronously.
/// A subsequent `get_job()` must return the config with `enabled == false`.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_get_job_reflects_set_enabled_false() {
    let (nats, _js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-get-enabled");

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: format!("cron.get-enabled.{id}") },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    client.set_enabled(&id, false).await.unwrap();

    let config = client.get_job(&id).await.unwrap()
        .expect("Job must exist after set_enabled");
    assert!(!config.enabled, "get_job must return enabled=false after set_enabled(false)");

    client.remove_job(&id).await.unwrap();
}

// ── NATS message subject matches the configured publish subject exactly ────────

/// The NATS message delivered for a publish tick must have its `subject`
/// field set to exactly the string configured in `Action::Publish`.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_tick_message_subject_matches_configured() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-msg-subject");
    let expected_subject = format!("cron.msg-subject.{id}");
    let mut sub = cron_messages(&nats, &js, expected_subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: expected_subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let msg = tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await.expect("Timed out").unwrap().unwrap();

    assert_eq!(
        msg.subject.as_str(), expected_subject,
        "NATS message subject must match the configured publish subject"
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── JSON null payload is normalised to None in the tick ──────────────────────

/// `payload: Some(Value::Null)` gets serialised as `"payload": null` in the
/// KV store; when deserialised back the serde pipeline normalises it to
/// `None`, so the tick's payload field is `None` — same as when no payload
/// was configured.  This test documents that observable behaviour.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_null_json_payload_normalises_to_none() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-null-payload");
    let subject = format!("cron.null-payload.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: Some(serde_json::Value::Null),
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let msg = tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await.expect("Timed out").unwrap().unwrap();
    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();

    // Some(Value::Null) round-trips as None through the serde pipeline.
    assert!(
        tick.payload.is_none(),
        "JSON null payload normalises to None in the tick, got: {:?}", tick.payload
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Two independent subscribers on the same subject both receive the tick ──────

/// Two separate JetStream consumers bound to the same publish subject must
/// both receive every tick — the scheduler publishes to the subject once and
/// NATS fans it out.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_multiple_subscribers_receive_same_tick() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-multi-sub");
    let subject = format!("cron.multi-sub.{id}");

    // Two independent subscribers.
    let mut sub1 = cron_messages(&nats, &js, subject.clone()).await;
    let mut sub2 = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let msg1 = tokio::time::timeout(Duration::from_secs(3), sub1.next())
        .await.expect("sub1 timed out").unwrap().unwrap();
    let msg2 = tokio::time::timeout(Duration::from_secs(3), sub2.next())
        .await.expect("sub2 timed out").unwrap().unwrap();

    let tick1: trogon_cron::TickPayload = serde_json::from_slice(&msg1.payload).unwrap();
    let tick2: trogon_cron::TickPayload = serde_json::from_slice(&msg2.payload).unwrap();

    assert_eq!(tick1.job_id, id, "sub1 tick must carry correct job_id");
    assert_eq!(tick2.job_id, id, "sub2 tick must carry correct job_id");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── interval_sec:2 produces ticks roughly 2 seconds apart ────────────────────

/// With `interval_sec: 2`, consecutive ticks must be separated by
/// approximately 2 seconds.  Collect three ticks and verify both gaps fall
/// within a generous 1–4 second window.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_interval_two_seconds_timing() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-interval-2s");
    let subject = format!("cron.interval-2s.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 2 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    let mut times = Vec::new();
    for i in 0..3usize {
        let msg = tokio::time::timeout(Duration::from_secs(8), sub.next())
            .await.unwrap_or_else(|_| panic!("Timed out on tick #{i}")).unwrap().unwrap();
        let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
        times.push(tick.fired_at);
    }

    for i in 1..times.len() {
        let gap = (times[i] - times[i - 1]).num_milliseconds().abs() as f64 / 1000.0;
        assert!(
            gap >= 1.0 && gap <= 4.0,
            "interval_sec:2 gap between tick[{}] and tick[{}] should be ~2 s, got {gap:.2} s",
            i - 1, i
        );
    }

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── set_enabled(true) on already-enabled job keeps it firing ─────────────────

/// Calling `set_enabled(true)` on a job that is already enabled must be a
/// safe no-op: the job's KV entry is updated (enabled stays true) and the
/// scheduler continues delivering ticks normally.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_set_enabled_true_on_already_enabled_continues() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-enable-idempotent");
    let subject = format!("cron.enable-idempotent.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Receive first tick to confirm job is active.
    tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await.expect("First tick must arrive").unwrap().unwrap();

    // Redundantly enable an already-enabled job.
    client.set_enabled(&id, true).await.unwrap();

    // Job must keep firing after the redundant enable call.
    tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await.expect("Job must keep firing after redundant set_enabled(true)").unwrap().unwrap();

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Scheduler skips spawn job whose binary does not exist ─────────────────────

/// A job whose `bin` path points to a file that does not exist on disk must
/// be skipped by the scheduler at startup (build_job_state will fail the
/// metadata check).  A valid job registered at the same time must still fire.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
#[cfg(unix)]
async fn test_spawn_nonexistent_bin_scheduler_skips_gracefully() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();

    let bad_id  = unique_id("test-nobin-bad");
    let good_id = unique_id("test-nobin-good");
    let subject = format!("cron.nobin-good.{good_id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    // Absolute path that does not exist — client accepts it, scheduler skips.
    client.register_job(&JobConfig {
        id: bad_id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/nonexistent/binary/trogon-test".to_string(),
            args: vec![],
            concurrent: false,
            timeout_sec: None,
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    // Valid publish job running alongside.
    client.register_job(&JobConfig {
        id: good_id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move {
        Scheduler::new(scheduler_nats).run().await.ok();
    });

    // Good job must fire — bad binary must not crash the scheduler.
    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await.expect("Good job must fire despite bad binary in another job")
        .unwrap().unwrap();
    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(tick.job_id, good_id);

    handle.abort();
    client.remove_job(&bad_id).await.unwrap();
    client.remove_job(&good_id).await.unwrap();
}

// ── Integer payload round-trips correctly ─────────────────────────────────────

/// `payload: Some(json!(42))` — a bare JSON integer — must survive the full
/// KV → scheduler → NATS → deserialisation pipeline unchanged.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_payload_integer_round_trips() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-int-payload");
    let subject = format!("cron.int-payload.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    let expected = serde_json::json!(42);

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: Some(expected.clone()),
        retry: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move { Scheduler::new(scheduler_nats).run().await.ok(); });

    let msg = tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await.expect("Timed out").unwrap().unwrap();
    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(tick.payload.unwrap(), expected, "Integer payload must round-trip unchanged");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Boolean payload round-trips correctly ─────────────────────────────────────

/// `payload: Some(json!(true))` — a bare JSON boolean — must survive the
/// full pipeline unchanged.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_payload_boolean_round_trips() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-bool-payload");
    let subject = format!("cron.bool-payload.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    let expected = serde_json::json!(true);

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: Some(expected.clone()),
        retry: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move { Scheduler::new(scheduler_nats).run().await.ok(); });

    let msg = tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await.expect("Timed out").unwrap().unwrap();
    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(tick.payload.unwrap(), expected, "Boolean payload must round-trip unchanged");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Leader failover: standby scheduler takes over after leader crashes ─────────

/// Start scheduler A (leader), confirm it fires, then abort it to simulate a
/// crash.  Reset the leader lock and start scheduler B.  B must win the
/// election and begin firing within 3 seconds — proving the standby failover
/// path works end-to-end.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_leader_failover_standby_takes_over() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-failover");
    let subject = format!("cron.failover.{id}");

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;

    // Start scheduler A and wait for it to become leader and fire.
    let mut sub_a = cron_messages(&nats, &js, subject.clone()).await;
    let nats_a = nats.clone();
    let handle_a = tokio::spawn(async move { Scheduler::new(nats_a).run().await.ok(); });

    tokio::time::timeout(Duration::from_secs(3), sub_a.next())
        .await.expect("Scheduler A must fire before crash").unwrap().unwrap();

    // Simulate leader crash.
    handle_a.abort();

    // Clear the stale lock so B can win immediately.
    reset_leader_lock(&js).await;

    // Start scheduler B (standby takes over).
    let mut sub_b = cron_messages(&nats, &js, subject.clone()).await;
    let nats_b = nats.clone();
    let handle_b = tokio::spawn(async move { Scheduler::new(nats_b).run().await.ok(); });

    tokio::time::timeout(Duration::from_secs(3), sub_b.next())
        .await.expect("Scheduler B must take over after A crashes").unwrap().unwrap();

    handle_b.abort();
    client.remove_job(&id).await.unwrap();
}

// ── concurrent:true spawns carry unique CRON_EXECUTION_IDs per invocation ──────

/// With `concurrent: true` and a 3-second sleeping process on a 1-second
/// interval, multiple invocations run simultaneously.  Each invocation writes
/// its own `$CRON_EXECUTION_ID` to a shared file.  After 4 seconds the file
/// must contain at least 2 distinct IDs, proving each spawn gets a fresh UUID.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_concurrent_true_spawns_have_unique_execution_ids() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-conc-execids");
    let out: PathBuf = std::env::temp_dir().join(format!("trogon-conc-execids-{id}"));
    let _ = std::fs::remove_file(&out);

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(),
                // Append execution_id then sleep so invocations overlap.
                format!("echo \"$CRON_EXECUTION_ID\" >> '{}'; sleep 3", out.to_str().unwrap())],
            concurrent: true,
            timeout_sec: Some(10),
        },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move { Scheduler::new(scheduler_nats).run().await.ok(); });

    tokio::time::sleep(Duration::from_secs(4)).await;

    let content = std::fs::read_to_string(&out).unwrap_or_default();
    let ids: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
    let unique: std::collections::HashSet<&str> = ids.iter().copied().collect();

    assert!(ids.len() >= 2, "concurrent:true must spawn ≥2 invocations in 4 s; got {}", ids.len());
    assert_eq!(ids.len(), unique.len(), "Every concurrent spawn must have a unique CRON_EXECUTION_ID");

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&out);
}

// ── Cron */5 expression fires at most once per 5-second window ─────────────────

/// `*/5 * * * * *` fires at seconds 0, 5, 10, 15 … of each minute.
/// Collect two consecutive ticks and verify the gap between their `fired_at`
/// timestamps is between 4 and 7 seconds.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_cron_step_every_5_seconds() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-cron-5s");
    let subject = format!("cron.cron5s.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Cron { expr: "*/5 * * * * *".to_string() },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move { Scheduler::new(scheduler_nats).run().await.ok(); });

    let msg1 = tokio::time::timeout(Duration::from_secs(10), sub.next())
        .await.expect("Timed out on first tick").unwrap().unwrap();
    let msg2 = tokio::time::timeout(Duration::from_secs(10), sub.next())
        .await.expect("Timed out on second tick").unwrap().unwrap();

    let t1: trogon_cron::TickPayload = serde_json::from_slice(&msg1.payload).unwrap();
    let t2: trogon_cron::TickPayload = serde_json::from_slice(&msg2.payload).unwrap();
    let gap = (t2.fired_at - t1.fired_at).num_milliseconds().abs() as f64 / 1000.0;

    assert!(
        gap >= 4.0 && gap <= 7.0,
        "*/5 cron should fire ~5 s apart, got {gap:.2} s"
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Float payload round-trips correctly ───────────────────────────────────────

/// `payload: Some(json!(3.14))` — a bare JSON floating-point number — must
/// survive the full KV → scheduler → NATS pipeline unchanged.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_payload_float_round_trips() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-float-payload");
    let subject = format!("cron.float-payload.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    let expected = serde_json::json!(3.14);

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: Some(expected.clone()),
        retry: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move { Scheduler::new(scheduler_nats).run().await.ok(); });

    let msg = tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await.expect("Timed out").unwrap().unwrap();
    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(tick.payload.unwrap(), expected, "Float payload must round-trip unchanged");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Bare string payload round-trips correctly ─────────────────────────────────

/// `payload: Some(json!("hello world"))` — a JSON string scalar, not wrapped
/// in an object — must survive the full pipeline unchanged.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_payload_bare_string_round_trips() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-str-payload");
    let subject = format!("cron.str-payload.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    let expected = serde_json::json!("hello world");

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: Some(expected.clone()),
        retry: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move { Scheduler::new(scheduler_nats).run().await.ok(); });

    let msg = tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await.expect("Timed out").unwrap().unwrap();
    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(tick.payload.unwrap(), expected, "Bare string payload must round-trip unchanged");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Disabling via register_job (not set_enabled) stops ticks ─────────────────

/// Call `register_job` directly with `enabled: false` while the scheduler
/// is running (as opposed to `set_enabled`).  This exercises the hot-reload
/// path for the `enabled` field and must silence the job within 3 seconds.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_disable_via_register_job_stops_firing() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-disable-via-register");
    let subject = format!("cron.disable-reg.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    let base_config = JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
};

    client.register_job(&base_config).await.unwrap();
    reset_leader_lock(&js).await;

    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move { Scheduler::new(scheduler_nats).run().await.ok(); });

    // Confirm job is firing.
    tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await.expect("Job must fire before disable").unwrap().unwrap();

    // Disable by re-registering with enabled:false.
    client.register_job(&JobConfig { enabled: false, ..base_config }).await.unwrap();

    // Drain any in-flight tick, then assert silence.
    tokio::time::sleep(Duration::from_millis(200)).await;
    while tokio::time::timeout(Duration::from_millis(100), sub.next()).await.is_ok() {}

    let still_firing = tokio::time::timeout(Duration::from_secs(2), sub.next()).await;
    assert!(still_firing.is_err(), "Job must stop firing after register_job with enabled:false");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── fired_at is close to the wall clock at the moment of receipt ──────────────

/// The `fired_at` timestamp in a tick must be within 5 seconds of the
/// actual wall-clock time when the message is received — confirming that
/// the scheduler stamps ticks with the real current time, not a stale value.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_fired_at_close_to_wall_clock() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-fired-at-wall");
    let subject = format!("cron.fired-at-wall.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
}).await.unwrap();

    reset_leader_lock(&js).await;
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move { Scheduler::new(scheduler_nats).run().await.ok(); });

    let before = chrono::Utc::now();
    let msg = tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await.expect("Timed out").unwrap().unwrap();
    let after = chrono::Utc::now();

    let tick: trogon_cron::TickPayload = serde_json::from_slice(&msg.payload).unwrap();

    assert!(
        tick.fired_at >= before - chrono::Duration::seconds(1)
            && tick.fired_at <= after + chrono::Duration::seconds(1),
        "fired_at {:?} must be within the observation window [{before:?}, {after:?}]",
        tick.fired_at
    );

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Registering the same config twice is idempotent ──────────────────────────

/// Calling `register_job` twice with identical configs must not cause double
/// firing — the scheduler treats the second KV put as a hot-reload to the
/// same state and fires exactly once per interval, not twice.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_register_same_config_twice_fires_once_per_tick() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-idempotent-reg");
    let subject = format!("cron.idempotent-reg.{id}");
    let mut sub = cron_messages(&nats, &js, subject.clone()).await;

    let config = JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: None,
        retry: None,
};

    // Register twice with identical config.
    client.register_job(&config).await.unwrap();
    client.register_job(&config).await.unwrap();

    reset_leader_lock(&js).await;
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move { Scheduler::new(scheduler_nats).run().await.ok(); });

    // Collect ticks over 3 seconds — must not get 2 ticks per scheduler tick.
    let mut count = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while let Ok(Some(Ok(_))) = tokio::time::timeout(
        deadline.saturating_duration_since(tokio::time::Instant::now()),
        sub.next(),
    ).await {
        count += 1;
    }

    // 1-second interval over 3 s → expect 2–4 ticks total, NOT 4–8 (double-fire).
    assert!(count <= 5, "Double-registration must not cause double-firing; got {count} ticks in 3 s");
    assert!(count >= 1, "Job must still fire after double-registration; got {count} ticks");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Retry policy: spawn retries on non-zero exit and eventually succeeds ──────

/// A spawn job with `max_retries: 2` runs a script that exits 1 on the first
/// attempt and exits 0 on the second.  A success-marker file must appear,
/// proving the retry loop re-executed the script.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_retry_succeeds_on_second_attempt() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-spawn-retry");

    let counter: std::path::PathBuf = std::env::temp_dir().join(format!("trogon-retry-counter-{id}"));
    let success: std::path::PathBuf = std::env::temp_dir().join(format!("trogon-retry-ok-{id}"));
    let _ = std::fs::remove_file(&counter);
    let _ = std::fs::remove_file(&success);

    // Script: increment a counter file; succeed only on the 2nd+ attempt.
    let script = format!(
        "count=$(cat '{}' 2>/dev/null || echo 0); count=$((count + 1)); printf '%s' \"$count\" > '{}'; [ \"$count\" -ge 2 ] && touch '{}' && exit 0; exit 1",
        counter.display(), counter.display(), success.display(),
    );

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
        retry: Some(trogon_cron::RetryConfig { max_retries: 2, retry_backoff_sec: 1, max_backoff_sec: None, max_retry_duration_sec: None }),
    }).await.unwrap();

    reset_leader_lock(&js).await;
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move { Scheduler::new(scheduler_nats).run().await.ok(); });

    // Wait up to 10 s for the success marker (actual time: ~2 s including 1 s backoff).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if success.exists() { break; }
        assert!(tokio::time::Instant::now() < deadline, "Success marker never created — retry did not succeed");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let count: u32 = std::fs::read_to_string(&counter).unwrap().trim().parse().unwrap();
    assert!(count >= 2, "Expected at least 2 spawn attempts, got {count}");

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&counter);
    let _ = std::fs::remove_file(&success);
}

// ── Retry policy: dead-letter published after all retries exhausted ───────────

/// A spawn job with `max_retries: 1` (2 total attempts) runs `exit 1` every
/// time.  After all attempts fail, the scheduler must publish a dead-letter
/// message to `cron.errors` containing the job_id, action type, and attempt count.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_dead_letter_after_exhausted_retries() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-dead-letter");

    // Subscribe to cron.errors BEFORE starting the scheduler using DeliverPolicy::New
    // so we don't receive dead-letters from previous test runs.
    let errors_inbox = nats.new_inbox();
    let mut errors = js.get_stream(trogon_cron::kv::TICKS_STREAM)
        .await.expect("CRON_TICKS stream not found")
        .create_consumer(consumer::push::Config {
            deliver_subject: errors_inbox,
            filter_subject: "cron.errors".to_string(),
            deliver_policy: consumer::DeliverPolicy::New,
            ack_policy: consumer::AckPolicy::None,
            ..Default::default()
        })
        .await.expect("Failed to create error consumer")
        .messages().await.expect("Failed to start error stream");

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "exit 1".to_string()],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: None,
        retry: Some(trogon_cron::RetryConfig { max_retries: 1, retry_backoff_sec: 1, max_backoff_sec: None, max_retry_duration_sec: None }),
    }).await.unwrap();

    reset_leader_lock(&js).await;
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move { Scheduler::new(scheduler_nats).run().await.ok(); });

    // max_retries=1 → 2 attempts: initial + 1 retry with 1s backoff → ~2 s total.
    // Wait up to 15 s to avoid flakiness.  Filter by job_id in case another test's
    // dead-letter arrives on the shared cron.errors subject.
    #[derive(serde::Deserialize)]
    struct DeadLetter { job_id: String, action: String, attempts: u32 }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let dl = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(remaining > Duration::ZERO, "Timed out waiting for dead-letter on cron.errors");
        let msg = tokio::time::timeout(remaining, errors.next())
            .await.expect("Timed out").unwrap().unwrap();
        let dl: DeadLetter = serde_json::from_slice(&msg.payload).unwrap();
        if dl.job_id == id { break dl; }
    };

    assert_eq!(dl.action, "spawn", "dead-letter action must be 'spawn'");
    assert_eq!(dl.attempts, 2, "expected 2 total attempts (initial + 1 retry)");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Retry policy: RetryConfig round-trips through NATS KV ────────────────────

/// Register a job with `retry: Some(RetryConfig { max_retries: 5, retry_backoff_sec: 3 })`,
/// read it back via `get_job`, and assert all fields survived the JSON ↔ KV round-trip.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_retry_config_round_trips_through_kv() {
    let (nats, _js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-retry-kv");

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 60 },
        action: Action::Publish { subject: format!("cron.{id}") },
        enabled: true,
        payload: None,
        retry: Some(trogon_cron::RetryConfig { max_retries: 5, retry_backoff_sec: 3, max_backoff_sec: None, max_retry_duration_sec: None }),
    }).await.unwrap();

    let fetched = client.get_job(&id).await.unwrap().expect("job must exist after register");
    let r = fetched.retry.expect("retry field must survive KV round-trip");
    assert_eq!(r.max_retries, 5);
    assert_eq!(r.retry_backoff_sec, 3);

    client.remove_job(&id).await.unwrap();
}

// ── max_retry_duration_sec stops retries before max_retries is reached ────────

/// A spawn job with `max_retries: 10` but `max_retry_duration_sec: 3` always
/// exits 1.  The duration cap must kick in and publish a dead-letter within
/// a few seconds — long before the 10-retry limit would be reached.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_dead_letter_respects_max_retry_duration() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-retry-dur");

    // Use DeliverPolicy::New to avoid receiving dead-letters from other tests.
    let errors_inbox = nats.new_inbox();
    let mut errors = js.get_stream(trogon_cron::kv::TICKS_STREAM)
        .await.expect("CRON_TICKS stream not found")
        .create_consumer(consumer::push::Config {
            deliver_subject: errors_inbox,
            filter_subject: "cron.errors".to_string(),
            deliver_policy: consumer::DeliverPolicy::New,
            ack_policy: consumer::AckPolicy::None,
            ..Default::default()
        })
        .await.expect("Failed to create error consumer")
        .messages().await.expect("Failed to start error stream");

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "exit 1".to_string()],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: None,
        retry: Some(trogon_cron::RetryConfig {
            max_retries: 10,
            retry_backoff_sec: 1, max_backoff_sec: None,
            max_retry_duration_sec: Some(3),
        }),
    }).await.unwrap();

    reset_leader_lock(&js).await;
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move { Scheduler::new(scheduler_nats).run().await.ok(); });

    // Duration cap is 3 s → dead-letter must arrive well before the 50-retry limit.
    // Allow 15 s total to avoid flakiness.
    #[derive(serde::Deserialize)]
    struct DeadLetter { job_id: String, attempts: u32 }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let dl = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(remaining > Duration::ZERO, "Timed out waiting for dead-letter");
        let msg = tokio::time::timeout(remaining, errors.next())
            .await.expect("Timed out").unwrap().unwrap();
        let dl: DeadLetter = serde_json::from_slice(&msg.payload).unwrap();
        if dl.job_id == id { break dl; }
    };

    // Must have stopped long before 10 retries.
    assert!(dl.attempts < 10, "Duration cap must stop retries before reaching max_retries; got {} attempts", dl.attempts);

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── max_retry_duration_sec round-trips through NATS KV ───────────────────────

/// Register a job with `max_retry_duration_sec: Some(120)`, read it back via
/// `get_job`, and assert the field survived the JSON ↔ KV round-trip.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_max_retry_duration_round_trips_through_kv() {
    let (nats, _js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-dur-kv");

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 60 },
        action: Action::Publish { subject: format!("cron.{id}") },
        enabled: true,
        payload: None,
        retry: Some(trogon_cron::RetryConfig {
            max_retries: 5,
            retry_backoff_sec: 2, max_backoff_sec: None,
            max_retry_duration_sec: Some(120),
        }),
    }).await.unwrap();

    let fetched = client.get_job(&id).await.unwrap().expect("job must exist");
    let r = fetched.retry.expect("retry must be present");
    assert_eq!(r.max_retries, 5);
    assert_eq!(r.retry_backoff_sec, 2);
    assert_eq!(r.max_retry_duration_sec, Some(120));

    client.remove_job(&id).await.unwrap();
}

// ── max_backoff_sec caps per-attempt delay ────────────────────────────────────

/// With `retry_backoff_sec=30` but `max_backoff_sec=2`, every retry delay is
/// capped at 2 s.  Without the cap, 3 retries of a failing spawn would take
/// >60 s; with the cap the dead-letter should arrive in under 15 s.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_max_backoff_sec_caps_delay() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-maxbo");

    let errors_consumer = js
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: "CRON_TICKS".to_string(),
            subjects: vec!["cron.>".to_string()],
            ..Default::default()
        })
        .await
        .unwrap()
        .create_consumer(async_nats::jetstream::consumer::push::Config {
            durable_name: Some(unique_id("c")),
            deliver_subject: nats.new_inbox(),
            deliver_policy: async_nats::jetstream::consumer::DeliverPolicy::New,
            filter_subject: "cron.errors".to_string(),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut errors = errors_consumer.messages().await.unwrap();

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/usr/bin/false".to_string(),
            args: vec![],
            concurrent: false,
            timeout_sec: None,
        },
        enabled: true,
        payload: None,
        retry: Some(trogon_cron::RetryConfig {
            max_retries: 3,
            retry_backoff_sec: 30,
            max_backoff_sec: Some(2),
            max_retry_duration_sec: None,
        }),
    }).await.unwrap();

    reset_leader_lock(&js).await;
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move { Scheduler::new(scheduler_nats).run().await.ok(); });

    // 3 retries × ≤2 s cap = ~6 s total; allow 15 s for flakiness.
    #[derive(serde::Deserialize)]
    struct DeadLetter { job_id: String, attempts: u32 }

    let start = tokio::time::Instant::now();
    let deadline = start + Duration::from_secs(15);
    let dl = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(remaining > Duration::ZERO, "Timed out waiting for dead-letter");
        let msg = tokio::time::timeout(remaining, errors.next())
            .await.expect("Timed out").unwrap().unwrap();
        let dl: DeadLetter = serde_json::from_slice(&msg.payload).unwrap();
        if dl.job_id == id { break dl; }
    };

    // Without the cap, retry_backoff_sec=30 → first retry alone would take 30 s.
    assert!(start.elapsed() < Duration::from_secs(14),
        "max_backoff_sec did not cap delays; elapsed {:?}", start.elapsed());
    assert_eq!(dl.attempts, 4, "expected 1 initial + 3 retries");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── max_backoff_sec round-trips through NATS KV ───────────────────────────────

/// Register a job with `max_backoff_sec: Some(30)`, read it back via
/// `get_job`, and assert the field survived the JSON ↔ KV round-trip.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_max_backoff_sec_round_trips_through_kv() {
    let (nats, _js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-maxbo-kv");

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 60 },
        action: Action::Publish { subject: format!("cron.{id}") },
        enabled: true,
        payload: None,
        retry: Some(trogon_cron::RetryConfig {
            max_retries: 5,
            retry_backoff_sec: 2,
            max_backoff_sec: Some(30),
            max_retry_duration_sec: None,
        }),
    }).await.unwrap();

    let fetched = client.get_job(&id).await.unwrap().expect("job must exist");
    let r = fetched.retry.expect("retry must be present");
    assert_eq!(r.max_retries, 5);
    assert_eq!(r.retry_backoff_sec, 2);
    assert_eq!(r.max_backoff_sec, Some(30));
    assert!(r.max_retry_duration_sec.is_none());

    client.remove_job(&id).await.unwrap();
}

// ── No dead-letter when retry is not configured ───────────────────────────────

/// A spawn job with no retry policy (`retry: None`) fails every tick.
/// No dead-letter must be published to `cron.errors` — silent failure is the
/// expected behaviour when the user has not opted in to retry tracking.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_no_dead_letter_without_retry_policy() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-no-dl");

    let errors_inbox = nats.new_inbox();
    let mut errors = js.get_stream(trogon_cron::kv::TICKS_STREAM)
        .await.expect("CRON_TICKS stream not found")
        .create_consumer(consumer::push::Config {
            deliver_subject: errors_inbox,
            filter_subject: "cron.errors".to_string(),
            deliver_policy: consumer::DeliverPolicy::New,
            ack_policy: consumer::AckPolicy::None,
            ..Default::default()
        })
        .await.expect("Failed to create error consumer")
        .messages().await.expect("Failed to start error stream");

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/usr/bin/false".to_string(),
            args: vec![],
            concurrent: false,
            timeout_sec: None,
        },
        enabled: true,
        payload: None,
        retry: None,
    }).await.unwrap();

    reset_leader_lock(&js).await;
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move { Scheduler::new(scheduler_nats).run().await.ok(); });

    // Wait 5 s and assert no dead-letter with our job_id arrives.
    #[derive(serde::Deserialize)]
    struct DeadLetter { job_id: String }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut got_our_dl = false;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() { break; }
        match tokio::time::timeout(remaining, errors.next()).await {
            Ok(Some(Ok(msg))) => {
                if let Ok(dl) = serde_json::from_slice::<DeadLetter>(&msg.payload) {
                    if dl.job_id == id { got_our_dl = true; break; }
                }
            }
            _ => break,
        }
    }

    assert!(!got_our_dl, "No dead-letter expected when retry policy is not set");
    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Job keeps firing after retry cycle is exhausted ──────────────────────────

/// A spawn job with `max_retries: 1` always fails.  After the first retry
/// cycle completes (2 attempts + dead-letter), `is_running` must be cleared
/// so the job fires again on the next tick.  We verify this by counting total
/// invocations: >= 4 means at least two full retry cycles ran.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_continues_firing_after_retry_exhausted() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-continues");

    let counter = std::env::temp_dir().join(format!("trogon-continues-{id}"));
    let _ = std::fs::remove_file(&counter);

    // Always exits 1, but counts every invocation so we can verify re-entry.
    let script = format!(
        "count=$(cat '{p}' 2>/dev/null || echo 0); printf '%s' \"$((count + 1))\" > '{p}'; exit 1",
        p = counter.display(),
    );

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script],
            concurrent: false,
            timeout_sec: Some(3),
        },
        enabled: true,
        payload: None,
        retry: Some(trogon_cron::RetryConfig {
            max_retries: 1,
            retry_backoff_sec: 1,
            max_backoff_sec: None,
            max_retry_duration_sec: None,
        }),
    }).await.unwrap();

    reset_leader_lock(&js).await;
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move { Scheduler::new(scheduler_nats).run().await.ok(); });

    // Two full cycles = 4 invocations (2 attempts per cycle).  Allow 20 s.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let count: u32 = std::fs::read_to_string(&counter)
            .ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        if count >= 4 { break; }
        assert!(tokio::time::Instant::now() < deadline,
            "Expected >= 4 invocations across two retry cycles, got {count}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&counter);
}

// ── max_retries wins when exhausted before max_retry_duration ─────────────────

/// With `max_retries: 2` and a generous `max_retry_duration_sec: 60`, the
/// retry count is exhausted first.  The dead-letter must report exactly
/// 3 attempts (1 initial + 2 retries), not fewer.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_max_retries_wins_over_duration() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-retries-win");

    let errors_inbox = nats.new_inbox();
    let mut errors = js.get_stream(trogon_cron::kv::TICKS_STREAM)
        .await.expect("CRON_TICKS stream not found")
        .create_consumer(consumer::push::Config {
            deliver_subject: errors_inbox,
            filter_subject: "cron.errors".to_string(),
            deliver_policy: consumer::DeliverPolicy::New,
            ack_policy: consumer::AckPolicy::None,
            ..Default::default()
        })
        .await.expect("Failed to create error consumer")
        .messages().await.expect("Failed to start error stream");

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/usr/bin/false".to_string(),
            args: vec![],
            concurrent: false,
            timeout_sec: None,
        },
        enabled: true,
        payload: None,
        retry: Some(trogon_cron::RetryConfig {
            max_retries: 2,
            retry_backoff_sec: 1,
            max_backoff_sec: None,
            max_retry_duration_sec: Some(60), // generous — max_retries hits first
        }),
    }).await.unwrap();

    reset_leader_lock(&js).await;
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move { Scheduler::new(scheduler_nats).run().await.ok(); });

    #[derive(serde::Deserialize)]
    struct DeadLetter { job_id: String, attempts: u32 }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let dl = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(remaining > Duration::ZERO, "Timed out waiting for dead-letter");
        let msg = tokio::time::timeout(remaining, errors.next())
            .await.expect("Timed out").unwrap().unwrap();
        let dl: DeadLetter = serde_json::from_slice(&msg.payload).unwrap();
        if dl.job_id == id { break dl; }
    };

    assert_eq!(dl.attempts, 3, "expected exactly 3 attempts (1 initial + 2 retries)");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Retry succeeds on the very last attempt ───────────────────────────────────

/// A spawn job with `max_retries: 3` fails the first 3 attempts and succeeds
/// only on the 4th (last allowed) attempt.  No dead-letter must be published
/// and the success marker must be created.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_retry_succeeds_on_last_attempt() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-last-attempt");

    let counter = std::env::temp_dir().join(format!("trogon-last-{id}"));
    let success = std::env::temp_dir().join(format!("trogon-last-ok-{id}"));
    let _ = std::fs::remove_file(&counter);
    let _ = std::fs::remove_file(&success);

    let script = format!(
        "count=$(cat '{c}' 2>/dev/null || echo 0); count=$((count+1)); printf '%s' \"$count\" > '{c}'; [ \"$count\" -ge 4 ] && touch '{s}' && exit 0; exit 1",
        c = counter.display(), s = success.display(),
    );

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 60 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script],
            concurrent: false,
            timeout_sec: Some(5),
        },
        enabled: true,
        payload: None,
        retry: Some(trogon_cron::RetryConfig {
            max_retries: 3,
            retry_backoff_sec: 1,
            max_backoff_sec: None,
            max_retry_duration_sec: None,
        }),
    }).await.unwrap();

    reset_leader_lock(&js).await;
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move { Scheduler::new(scheduler_nats).run().await.ok(); });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if success.exists() { break; }
        assert!(tokio::time::Instant::now() < deadline,
            "Success marker never created — last-attempt retry did not succeed");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let count: u32 = std::fs::read_to_string(&counter).unwrap().trim().parse().unwrap();
    assert_eq!(count, 4, "Expected exactly 4 invocations (1 initial + 3 retries)");

    handle.abort();
    client.remove_job(&id).await.unwrap();
    let _ = std::fs::remove_file(&counter);
    let _ = std::fs::remove_file(&success);
}

// ── Timeout triggers retry ────────────────────────────────────────────────────

/// A spawn job whose process always sleeps beyond its `timeout_sec` budget.
/// Each attempt must time out and be retried.  After exhausting `max_retries`,
/// a dead-letter must be published with action="spawn" and the correct attempt count.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_timeout_triggers_retry() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-timeout-retry");

    let errors_inbox = nats.new_inbox();
    let mut errors = js.get_stream(trogon_cron::kv::TICKS_STREAM)
        .await.expect("CRON_TICKS stream not found")
        .create_consumer(consumer::push::Config {
            deliver_subject: errors_inbox,
            filter_subject: "cron.errors".to_string(),
            deliver_policy: consumer::DeliverPolicy::New,
            ack_policy: consumer::AckPolicy::None,
            ..Default::default()
        })
        .await.expect("Failed to create error consumer")
        .messages().await.expect("Failed to start error stream");

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 60 },
        action: Action::Spawn {
            bin: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "sleep 30".to_string()],
            concurrent: false,
            timeout_sec: Some(1),
        },
        enabled: true,
        payload: None,
        retry: Some(trogon_cron::RetryConfig {
            max_retries: 2,
            retry_backoff_sec: 1,
            max_backoff_sec: None,
            max_retry_duration_sec: None,
        }),
    }).await.unwrap();

    reset_leader_lock(&js).await;
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move { Scheduler::new(scheduler_nats).run().await.ok(); });

    #[derive(serde::Deserialize)]
    struct DeadLetter { job_id: String, action: String, attempts: u32 }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    let dl = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(remaining > Duration::ZERO, "Timed out waiting for dead-letter");
        let msg = tokio::time::timeout(remaining, errors.next())
            .await.expect("Timed out").unwrap().unwrap();
        let dl: DeadLetter = serde_json::from_slice(&msg.payload).unwrap();
        if dl.job_id == id { break dl; }
    };

    assert_eq!(dl.action, "spawn");
    assert_eq!(dl.attempts, 3, "expected 3 attempts (1 initial + 2 retries after timeout)");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Dead-letter payload contains all expected fields ─────────────────────────

/// After retries are exhausted, the dead-letter published to `cron.errors`
/// must contain: job_id, execution_id (non-empty), fired_at, action, attempts,
/// and error (non-empty).
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_dead_letter_payload_fields_are_correct() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-dl-fields");

    let errors_inbox = nats.new_inbox();
    let mut errors = js.get_stream(trogon_cron::kv::TICKS_STREAM)
        .await.expect("CRON_TICKS stream not found")
        .create_consumer(consumer::push::Config {
            deliver_subject: errors_inbox,
            filter_subject: "cron.errors".to_string(),
            deliver_policy: consumer::DeliverPolicy::New,
            ack_policy: consumer::AckPolicy::None,
            ..Default::default()
        })
        .await.expect("Failed to create error consumer")
        .messages().await.expect("Failed to start error stream");

    let before = chrono::Utc::now();

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 60 },
        action: Action::Spawn {
            bin: "/usr/bin/false".to_string(),
            args: vec![],
            concurrent: false,
            timeout_sec: None,
        },
        enabled: true,
        payload: None,
        retry: Some(trogon_cron::RetryConfig {
            max_retries: 1,
            retry_backoff_sec: 1,
            max_backoff_sec: None,
            max_retry_duration_sec: None,
        }),
    }).await.unwrap();

    reset_leader_lock(&js).await;
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move { Scheduler::new(scheduler_nats).run().await.ok(); });

    #[derive(serde::Deserialize)]
    struct DeadLetter {
        job_id: String,
        execution_id: String,
        fired_at: chrono::DateTime<chrono::Utc>,
        action: String,
        attempts: u32,
        error: String,
    }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let dl = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(remaining > Duration::ZERO, "Timed out waiting for dead-letter");
        let msg = tokio::time::timeout(remaining, errors.next())
            .await.expect("Timed out").unwrap().unwrap();
        let dl: DeadLetter = serde_json::from_slice(&msg.payload).unwrap();
        if dl.job_id == id { break dl; }
    };

    assert_eq!(dl.job_id, id);
    assert!(!dl.execution_id.is_empty(), "execution_id must not be empty");
    assert!(dl.fired_at >= before, "fired_at must be >= time before job registered");
    assert_eq!(dl.action, "spawn");
    assert_eq!(dl.attempts, 2);
    assert!(!dl.error.is_empty(), "error field must not be empty");

    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── Explicit max_retries=0 → no dead-letter ───────────────────────────────────

/// Setting `retry: Some(RetryConfig { max_retries: 0, .. })` explicitly must
/// behave identically to `retry: None` — no dead-letter is published.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_explicit_max_retries_zero_no_dead_letter() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-retries-zero");

    let errors_inbox = nats.new_inbox();
    let mut errors = js.get_stream(trogon_cron::kv::TICKS_STREAM)
        .await.expect("CRON_TICKS stream not found")
        .create_consumer(consumer::push::Config {
            deliver_subject: errors_inbox,
            filter_subject: "cron.errors".to_string(),
            deliver_policy: consumer::DeliverPolicy::New,
            ack_policy: consumer::AckPolicy::None,
            ..Default::default()
        })
        .await.expect("Failed to create error consumer")
        .messages().await.expect("Failed to start error stream");

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Spawn {
            bin: "/usr/bin/false".to_string(),
            args: vec![],
            concurrent: false,
            timeout_sec: None,
        },
        enabled: true,
        payload: None,
        retry: Some(trogon_cron::RetryConfig {
            max_retries: 0,
            retry_backoff_sec: 1,
            max_backoff_sec: None,
            max_retry_duration_sec: None,
        }),
    }).await.unwrap();

    reset_leader_lock(&js).await;
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move { Scheduler::new(scheduler_nats).run().await.ok(); });

    #[derive(serde::Deserialize)]
    struct DeadLetter { job_id: String }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut got_our_dl = false;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() { break; }
        match tokio::time::timeout(remaining, errors.next()).await {
            Ok(Some(Ok(msg))) => {
                if let Ok(dl) = serde_json::from_slice::<DeadLetter>(&msg.payload) {
                    if dl.job_id == id { got_our_dl = true; break; }
                }
            }
            _ => break,
        }
    }

    assert!(!got_our_dl, "No dead-letter expected when max_retries=0");
    handle.abort();
    client.remove_job(&id).await.unwrap();
}

// ── max_backoff_sec and max_retry_duration both active ────────────────────────

/// With `retry_backoff_sec=30, max_backoff_sec=2, max_retry_duration_sec=8`,
/// every delay is capped at 2 s by max_backoff_sec.  After ~8 s the duration
/// budget stops the loop before max_retries (10) is reached.
#[tokio::test]
#[ignore = "requires NATS at NATS_TEST_URL"]
async fn test_spawn_max_backoff_and_duration_both_active() {
    let (nats, js) = connect_js().await;
    let client = CronClient::new(nats.clone()).await.unwrap();
    let id = unique_id("test-both-caps");

    let errors_inbox = nats.new_inbox();
    let mut errors = js.get_stream(trogon_cron::kv::TICKS_STREAM)
        .await.expect("CRON_TICKS stream not found")
        .create_consumer(consumer::push::Config {
            deliver_subject: errors_inbox,
            filter_subject: "cron.errors".to_string(),
            deliver_policy: consumer::DeliverPolicy::New,
            ack_policy: consumer::AckPolicy::None,
            ..Default::default()
        })
        .await.expect("Failed to create error consumer")
        .messages().await.expect("Failed to start error stream");

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 60 },
        action: Action::Spawn {
            bin: "/usr/bin/false".to_string(),
            args: vec![],
            concurrent: false,
            timeout_sec: None,
        },
        enabled: true,
        payload: None,
        retry: Some(trogon_cron::RetryConfig {
            max_retries: 10,
            retry_backoff_sec: 30,
            max_backoff_sec: Some(2),
            max_retry_duration_sec: Some(8),
        }),
    }).await.unwrap();

    reset_leader_lock(&js).await;
    let start = tokio::time::Instant::now();
    let scheduler_nats = nats.clone();
    let handle = tokio::spawn(async move { Scheduler::new(scheduler_nats).run().await.ok(); });

    #[derive(serde::Deserialize)]
    struct DeadLetter { job_id: String, attempts: u32 }

    let deadline = start + Duration::from_secs(20);
    let dl = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(remaining > Duration::ZERO, "Timed out waiting for dead-letter");
        let msg = tokio::time::timeout(remaining, errors.next())
            .await.expect("Timed out").unwrap().unwrap();
        let dl: DeadLetter = serde_json::from_slice(&msg.payload).unwrap();
        if dl.job_id == id { break dl; }
    };

    assert!(dl.attempts < 10,
        "duration cap must stop before max_retries; got {} attempts", dl.attempts);
    assert!(start.elapsed() < Duration::from_secs(18),
        "took too long — max_backoff_sec did not cap delays: {:?}", start.elapsed());

    handle.abort();
    client.remove_job(&id).await.unwrap();
}
