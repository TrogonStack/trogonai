//! Integration tests — require a running NATS server.
//!
//! Run with:
//!   NATS_TEST_URL=nats://localhost:4222 cargo test -p trogon-cron -- --include-ignored
//!
//! These tests are marked `#[ignore]` so they don't run in CI without NATS.

use std::time::Duration;

use async_nats::jetstream::{self, consumer};
use futures::StreamExt;
use trogon_cron::{Action, CronClient, JobConfig, Schedule, Scheduler};

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

/// Create an ephemeral JetStream pull consumer on CRON_TICKS filtered to
/// `subject` and return a continuous message stream.
async fn cron_messages(
    js: &jetstream::Context,
    subject: String,
) -> impl futures::Stream<Item = Result<jetstream::Message, consumer::pull::MessagesError>> {
    js.get_stream(trogon_cron::kv::TICKS_STREAM)
        .await
        .expect("CRON_TICKS stream not found")
        .create_consumer(consumer::pull::Config {
            filter_subject: subject,
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
        action: Action::Publish { subject: "test.tick".to_string() },
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
        action: Action::Publish { subject: "test.get".to_string() },
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
        action: Action::Publish { subject: "test.toggle".to_string() },
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

    // Subscribe via JetStream pull consumer — backed by CRON_TICKS stream,
    // so ticks are persisted and delivered at-least-once.
    let mut sub = cron_messages(&js, subject.clone()).await;

    let job = JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: true,
        payload: Some(serde_json::json!({ "test": true })),
    };
    client.register_job(&job).await.unwrap();

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

    let mut sub_v2 = cron_messages(&js, subject_v2.clone()).await;

    // Register v1 job
    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject_v1.clone() },
        enabled: true,
        payload: None,
    }).await.unwrap();

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

    let mut sub = cron_messages(&js, subject.clone()).await;

    client.register_job(&JobConfig {
        id: id.clone(),
        schedule: Schedule::Interval { interval_sec: 1 },
        action: Action::Publish { subject: subject.clone() },
        enabled: false,
        payload: None,
    }).await.unwrap();

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
