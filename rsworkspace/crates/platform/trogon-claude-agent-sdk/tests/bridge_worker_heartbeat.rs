//! The heartbeat timer replays the timelines the pinned upstream bundle produced with its timers and `Math.random`
//! stubbed (`tests/fixtures/bridge_worker/timelines.json`): the virtual time of every schedule, request and close, and
//! the random values the schedule drew. The worker here talks to an in-memory service, so the Tokio clock is paused
//! and the timeline is exact.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::ResponseBuilderExt;
use serde_json::Value;
use tokio::time::Instant;
use trogon_claude_agent_sdk::adapters::HttpExecute;
use trogon_claude_agent_sdk::{
    HeartbeatDriver, NonOrigin403, SensitiveString, WorkerClient, WorkerConfig, WorkerPacing, heartbeat_delay,
};

const TIMELINES: &str = include_str!("fixtures/bridge_worker/timelines.json");

type Log = Arc<Mutex<Vec<String>>>;

fn stamp(start: Instant) -> u128 {
    start.elapsed().as_millis()
}

fn note(log: &Log, start: Instant, line: &str) {
    log.lock().unwrap().push(format!("{} {line}", stamp(start)));
}

/// The service: answers `200 {}` unless the case scripts the n-th heartbeat answer, and logs what it is asked.
struct Service {
    start: Instant,
    log: Log,
    answers: Vec<Value>,
    beats: AtomicUsize,
}

impl HttpExecute for Service {
    fn execute(&self, request: reqwest::Request) -> impl Future<Output = reqwest::Result<reqwest::Response>> + Send {
        let route = request
            .url()
            .path()
            .trim_start_matches("/v1/code/sessions/cse_abc")
            .to_owned();
        let method = request.method().to_string();
        if !(method == "GET" && route == "/worker") {
            note(&self.log, self.start, &format!("request {method} {route}"));
        }
        let scripted = if route == "/worker/heartbeat" {
            self.answers.get(self.beats.fetch_add(1, Ordering::SeqCst)).cloned()
        } else {
            None
        };
        let url = request.url().clone();
        async move {
            let (status, body) = match scripted {
                Some(Value::String(_)) => std::future::pending().await,
                Some(answer) => (answer["status"].as_u64().unwrap_or(200), answer["body"].clone()),
                None => (200, serde_json::json!({})),
            };
            let response = http::Response::builder()
                .status(u16::try_from(status).unwrap_or(200))
                .header("content-type", "application/json")
                .url(url)
                .body(body.to_string())
                .expect("a response");
            Ok(reqwest::Response::from(response))
        }
    }
}

/// The timer: draws the recorded random values in order and logs each delay it is asked to wait.
struct Recorded {
    start: Instant,
    log: Log,
    randoms: Mutex<VecDeque<f64>>,
}

impl WorkerPacing for Recorded {
    fn jitter(&self) -> f64 {
        self.randoms
            .lock()
            .unwrap()
            .pop_front()
            .expect("the worker drew more random values than the bundle did")
    }

    fn pause(&self, delay: Duration) -> impl Future<Output = ()> + Send {
        note(&self.log, self.start, &format!("schedule {}", delay.as_millis()));
        tokio::time::sleep(delay)
    }
}

async fn replay(name: &str) {
    let all: Value = serde_json::from_str(TIMELINES).expect("timelines parse");
    let case = all["timelines"]
        .as_array()
        .and_then(|cases| cases.iter().find(|case| case["name"] == name))
        .expect("a recorded case");
    let script = &case["script"];
    let until = script["until"].as_u64().expect("until");
    let start = Instant::now();
    let log: Log = Arc::default();
    let randoms: VecDeque<f64> = case["randoms"]
        .as_array()
        .expect("randoms")
        .iter()
        .map(|value| value.as_f64().expect("a number"))
        .collect();
    let service = Service {
        start,
        log: Arc::clone(&log),
        answers: script["answers"].as_array().cloned().unwrap_or_default(),
        beats: AtomicUsize::new(0),
    };
    let pacing = Recorded {
        start,
        log: Arc::clone(&log),
        randoms: Mutex::new(randoms),
    };
    let client = Arc::new(
        WorkerClient::new(
            service,
            pacing,
            WorkerConfig {
                api_base_url: "http://service.invalid".to_owned(),
                session_id: "cse_abc".to_owned(),
                token: SensitiveString::from("tok"),
                entrypoint: None,
                heartbeat_interval_ms: script["interval"].as_u64(),
                session_access_token: None,
                non_origin_403: NonOrigin403::default(),
            },
        )
        .expect("a worker"),
    );
    client.initialize(2).await.expect("registered");
    let closed = Arc::new(AtomicBool::new(false));
    let close_log = Arc::clone(&log);
    let driver = HeartbeatDriver::start(Arc::clone(&client), Arc::clone(&closed), move |reason| {
        note(&close_log, start, &format!("closed {}", reason.close_code(false)));
    });
    if let Some(close_at) = script["close_at"].as_u64() {
        tokio::time::sleep_until(start + Duration::from_millis(close_at)).await;
        driver.stop();
        closed.store(true, Ordering::Release);
        assert!(client.goodbye().await);
    }
    tokio::time::sleep_until(start + Duration::from_millis(until + 1)).await;
    drop(driver);
    let recorded: Vec<String> = serde_json::from_value(case["timeline"].clone()).expect("a timeline");
    assert_eq!(log.lock().unwrap().clone(), recorded, "{name}");
}

#[tokio::test(start_paused = true)]
async fn the_default_interval_beats_every_twenty_seconds() {
    replay("the_default_interval_beats_every_twenty_seconds").await;
}

#[tokio::test(start_paused = true)]
async fn an_advised_longer_interval_applies_from_the_next_schedule() {
    replay("an_advised_longer_interval_applies_from_the_next_schedule").await;
}

#[tokio::test(start_paused = true)]
async fn an_advised_shorter_interval_restarts_the_timer() {
    replay("an_advised_shorter_interval_restarts_the_timer").await;
}

#[tokio::test(start_paused = true)]
async fn an_advised_interval_below_the_floor_is_raised_to_it() {
    replay("an_advised_interval_below_the_floor_is_raised_to_it").await;
}

#[tokio::test(start_paused = true)]
async fn an_advised_interval_above_the_ceiling_is_lowered_to_it() {
    replay("an_advised_interval_above_the_ceiling_is_lowered_to_it").await;
}

#[tokio::test(start_paused = true)]
async fn an_advised_interval_that_is_not_positive_changes_nothing() {
    replay("an_advised_interval_that_is_not_positive_changes_nothing").await;
}

#[tokio::test(start_paused = true)]
async fn a_beat_is_skipped_while_the_previous_one_is_unanswered() {
    replay("a_beat_is_skipped_while_the_previous_one_is_unanswered").await;
}

#[tokio::test(start_paused = true)]
async fn a_given_interval_below_the_floor_is_kept_until_the_service_advises_one() {
    replay("a_given_interval_below_the_floor_is_kept_until_the_service_advises_one").await;
}

#[tokio::test(start_paused = true)]
async fn closing_stops_the_timer() {
    replay("closing_stops_the_timer").await;
}

#[tokio::test(start_paused = true)]
async fn a_terminal_answer_stops_the_timer() {
    replay("a_terminal_answer_stops_the_timer").await;
}

/// The session raising its closed flag ends the timer without the driver being told: the next tick sees it and stops.
#[tokio::test(start_paused = true)]
async fn the_closed_flag_ends_the_timer_at_the_next_tick() {
    let start = Instant::now();
    let log: Log = Arc::default();
    let client = Arc::new(
        WorkerClient::new(
            Service {
                start,
                log: Arc::clone(&log),
                answers: Vec::new(),
                beats: AtomicUsize::new(0),
            },
            Recorded {
                start,
                log: Arc::clone(&log),
                randoms: Mutex::new(VecDeque::from([0.5])),
            },
            WorkerConfig {
                api_base_url: "http://service.invalid".to_owned(),
                session_id: "cse_abc".to_owned(),
                token: SensitiveString::from("tok"),
                entrypoint: None,
                heartbeat_interval_ms: None,
                session_access_token: None,
                non_origin_403: NonOrigin403::default(),
            },
        )
        .expect("a worker"),
    );
    let closed = Arc::new(AtomicBool::new(false));
    let driver = HeartbeatDriver::start(client, Arc::clone(&closed), |_| {});
    tokio::time::sleep(Duration::from_millis(1)).await;
    closed.store(true, Ordering::Release);
    tokio::time::sleep(Duration::from_secs(60)).await;
    drop(driver);
    assert_eq!(log.lock().unwrap().clone(), vec!["0 schedule 20000".to_owned()]);
}

#[test]
fn the_delay_is_the_interval_when_nothing_spreads_it() {
    assert_eq!(heartbeat_delay(20_000.0, 0.0, 0.0, 0.9), Duration::from_millis(20_000));
}

#[test]
fn the_spread_is_a_fraction_of_the_interval_around_it() {
    assert_eq!(heartbeat_delay(20_000.0, 0.1, 0.0, 0.0), Duration::from_millis(18_000));
    assert_eq!(heartbeat_delay(20_000.0, 0.1, 0.0, 0.5), Duration::from_millis(20_000));
    assert_eq!(heartbeat_delay(20_000.0, 0.1, 0.0, 1.0), Duration::from_millis(22_000));
}

#[test]
fn the_spread_is_at_least_the_floor() {
    assert_eq!(
        heartbeat_delay(20_000.0, 0.01, 2_000.0, 1.0),
        Duration::from_millis(22_000)
    );
}

#[test]
fn the_delay_is_at_most_the_largest_timer() {
    assert_eq!(
        heartbeat_delay(3_000_000_000.0, 0.0, 0.0, 0.5),
        Duration::from_millis(2_147_483_647)
    );
}

#[test]
fn a_delay_below_one_millisecond_is_one_millisecond() {
    assert_eq!(heartbeat_delay(0.2, 0.0, 0.0, 0.5), Duration::from_millis(1));
    assert_eq!(heartbeat_delay(1_000.0, 2.0, 0.0, 0.0), Duration::from_millis(1));
}
