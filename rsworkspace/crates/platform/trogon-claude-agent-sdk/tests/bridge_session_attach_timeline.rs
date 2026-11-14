//! A default-option attach schedules its own heartbeat, uploads and read stream, and replays the timelines the pinned
//! upstream bundle produced with its timers and `Math.random` stubbed (`stream` cases of
//! `tests/fixtures/bridge_worker/timelines.json`): when each request for the stream and each heartbeat was sent and
//! when the session closed. The service is scripted from the case, the Tokio clock is paused and the random draws
//! are the recorded ones, so the times are exact. A reconnect replays `reconnectTransport` of the default attach.
//! The cases that move the clocks (`skews`) hand the attach clocks that jump ahead at their times, the way a paused
//! or sleeping process does.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::ResponseBuilderExt;
use serde_json::{Value, json};
use tokio::time::Instant;
use trogon_claude_agent_sdk::adapters::HttpExecute;
use trogon_claude_agent_sdk::{
    AttachBridgeSessionOptions, ReconnectOptions, StreamClocks, WorkerPacing, attach_bridge_session,
};

/// What `Date.now` reads at the start of the case; only the differences matter.
const WALL_ORIGIN: Duration = Duration::from_secs(1_700_000_000);

const TIMELINES: &str = include_str!("fixtures/bridge_worker/timelines.json");

type Log = Arc<Mutex<Vec<String>>>;

fn note(log: &Log, start: Instant, line: &str) {
    log.lock()
        .unwrap()
        .push(format!("{} {line}", start.elapsed().as_millis()));
}

#[derive(Clone, Copy)]
enum Finish {
    End,
    Reset,
    Hold,
}

/// A body whose chunks arrive at their times and which then ends, breaks or stays open.
fn timed_body(chunks: VecDeque<(Instant, Vec<u8>)>, finish: Finish) -> reqwest::Body {
    let stream = futures_util::stream::unfold((chunks, finish), |(mut chunks, finish)| async move {
        if let Some((at, bytes)) = chunks.pop_front() {
            tokio::time::sleep_until(at).await;
            return Some((Ok::<_, std::io::Error>(bytes), (chunks, finish)));
        }
        match finish {
            Finish::End => None,
            Finish::Reset => Some((Err(std::io::Error::other("reset")), (chunks, Finish::End))),
            Finish::Hold => std::future::pending().await,
        }
    });
    reqwest::Body::wrap_stream(stream)
}

struct Inner {
    start: Instant,
    log: Log,
    conns: Vec<Value>,
    answers: Vec<Value>,
    streams: AtomicUsize,
    beats: AtomicUsize,
}

/// The service of the case: the n-th request for the stream gets the n-th scripted connection (a request past the
/// script gets a stream that stays open and silent), the heartbeats the case scripts as `hang` are never answered,
/// and every other request is answered `200 {}`.
#[derive(Clone)]
struct Service(Arc<Inner>);

impl HttpExecute for Service {
    fn execute(&self, request: reqwest::Request) -> impl Future<Output = reqwest::Result<reqwest::Response>> + Send {
        let inner = Arc::clone(&self.0);
        let url = request.url().clone();
        let path = url.path().to_owned();
        let mut conn = None;
        let mut hang = false;
        if path.ends_with("/worker/heartbeat") {
            note(&inner.log, inner.start, "request POST /worker/heartbeat");
            hang = inner.answers.get(inner.beats.fetch_add(1, Ordering::SeqCst)).is_some();
        } else if path.ends_with("/worker/events/stream") {
            // The recorder logged the stream's route with its query, so a resumed open shows where it resumes from.
            let query = url.query().map(|query| format!("?{query}")).unwrap_or_default();
            note(
                &inner.log,
                inner.start,
                &format!("request GET /worker/events/stream{query}"),
            );
            conn = Some(
                inner
                    .conns
                    .get(inner.streams.fetch_add(1, Ordering::SeqCst))
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            );
        }
        async move {
            if hang || conn.as_ref().is_some_and(|conn| conn == "hang") {
                std::future::pending::<()>().await;
            }
            let Some(conn) = conn else {
                let response = http::Response::builder()
                    .status(200)
                    .header("content-type", "application/json")
                    .url(url)
                    .body(reqwest::Body::from("{}"))
                    .expect("a response");
                return Ok(reqwest::Response::from(response));
            };
            if conn.is_string() {
                return reqwest::Client::new().get("not a url").build().map(|_| unreachable!());
            }
            let status = u16::try_from(conn["status"].as_u64().unwrap_or(200)).expect("a status");
            let now = Instant::now();
            let chunks: VecDeque<(Instant, Vec<u8>)> = conn["chunks"]
                .as_array()
                .map(|chunks| {
                    chunks
                        .iter()
                        .map(|chunk| {
                            let delay = chunk[0].as_u64().expect("a delay");
                            let text = chunk[1].as_str().expect("a text");
                            (now + Duration::from_millis(delay), text.as_bytes().to_vec())
                        })
                        .collect()
                })
                .unwrap_or_default();
            let finish = match conn["then"].as_str() {
                Some("end") => Finish::End,
                Some(_) => Finish::Reset,
                None if status >= 400 && conn["hangBody"] != true => Finish::End,
                None => Finish::Hold,
            };
            let response = http::Response::builder()
                .status(status)
                .url(url)
                .body(timed_body(chunks, finish))
                .expect("a response");
            Ok(reqwest::Response::from(response))
        }
    }
}

/// Draws the recorded random values in the order the bundle drew them and waits on the Tokio timer.
#[derive(Clone)]
struct Recorded(Arc<Mutex<VecDeque<f64>>>);

impl WorkerPacing for Recorded {
    fn jitter(&self) -> f64 {
        self.0
            .lock()
            .unwrap()
            .pop_front()
            .expect("the session drew more random values than the bundle did")
    }

    fn pause(&self, delay: Duration) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(delay)
    }
}

fn draws(case: &Value, key: &str) -> Vec<f64> {
    case[key]
        .as_array()
        .expect("randoms")
        .iter()
        .map(|value| value.as_f64().expect("a number"))
        .collect()
}

async fn replay(name: &str) {
    let all: Value = serde_json::from_str(TIMELINES).expect("timelines parse");
    let case = all["timelines"]
        .as_array()
        .and_then(|cases| cases.iter().find(|case| case["name"] == name))
        .cloned()
        .expect("a recorded case");
    let script = &case["script"];
    let until = script["until"].as_u64().expect("until");
    let start = Instant::now();
    let log: Log = Arc::default();
    let service = Service(Arc::new(Inner {
        start,
        log: Arc::clone(&log),
        conns: script["conns"].as_array().cloned().unwrap_or_default(),
        answers: script["answers"].as_array().cloned().unwrap_or_default(),
        streams: AtomicUsize::new(0),
        beats: AtomicUsize::new(0),
    }));
    // The heartbeat schedule draws first, at the attach; every other draw is the stream's.
    let mut randoms = draws(&case, "randoms");
    randoms.extend(draws(&case, "stream_randoms"));
    let mut options = AttachBridgeSessionOptions::new("cse_abc", "http://service.invalid", "tok");
    options.epoch = Some(2);
    options.heartbeat_interval_ms = script["interval"].as_u64();
    options.arguments = Some(Vec::new());
    options.environment = Some(Arc::new(|_| None));
    let wall_skew = Arc::new(AtomicU64::new(0));
    let monotonic_skew = Arc::new(AtomicU64::new(0));
    let (wall, monotonic) = (Arc::clone(&wall_skew), Arc::clone(&monotonic_skew));
    options.stream_clocks = Some(StreamClocks {
        wall: Arc::new(move || WALL_ORIGIN + start.elapsed() + Duration::from_millis(wall.load(Ordering::SeqCst))),
        monotonic: Arc::new(move || start.elapsed() + Duration::from_millis(monotonic.load(Ordering::SeqCst))),
    });
    let skews = script["skews"].as_array().cloned().unwrap_or_default();
    let jumper = tokio::spawn(async move {
        for skew in skews {
            tokio::time::sleep_until(start + Duration::from_millis(skew[0].as_u64().expect("a time"))).await;
            wall_skew.fetch_add(skew[1].as_u64().expect("wall"), Ordering::SeqCst);
            monotonic_skew.fetch_add(skew[2].as_u64().expect("monotonic"), Ordering::SeqCst);
        }
    });
    let closing = Arc::clone(&log);
    options.callbacks.on_close = Some(Box::new(move |code| note(&closing, start, &format!("closed {code}"))));
    let mut session = attach_bridge_session(service, Recorded(Arc::new(Mutex::new(randoms.into()))), options)
        .await
        .expect("attached");

    let mut calls: Vec<(u64, &str)> = ["reconnect_at", "close_at"]
        .into_iter()
        .filter_map(|key| script[key].as_u64().map(|at| (at, key)))
        .collect();
    calls.sort_by_key(|(at, _)| *at);
    for (at, call) in calls {
        tokio::time::sleep_until(start + Duration::from_millis(at)).await;
        if call == "close_at" {
            session.close().await;
        } else {
            let reconnect = ReconnectOptions {
                api_base_url: "http://service.invalid".to_owned(),
                ingress_token: "tok2".into(),
                epoch: None,
            };
            session.reconnect_transport(reconnect).await.expect("reconnected");
        }
    }
    tokio::time::sleep_until(start + Duration::from_millis(until + 1)).await;
    drop(session);
    jumper.await.expect("the clocks jumped");
    let recorded: Vec<String> = serde_json::from_value(case["timeline"].clone()).expect("a timeline");
    let kept: Vec<String> = recorded
        .into_iter()
        .filter(|line| {
            [
                " request GET /worker/events/stream",
                " request POST /worker/heartbeat",
                " closed ",
            ]
            .iter()
            .any(|part| line.contains(part))
        })
        .collect();
    assert_eq!(log.lock().unwrap().clone(), kept, "{name}");
}

macro_rules! recorded {
    ($($name:ident),+ $(,)?) => {
        $(
            #[tokio::test(start_paused = true)]
            async fn $name() {
                replay(stringify!($name)).await;
            }
        )+
    };
}

recorded!(
    a_response_that_never_comes_is_given_up_after_thirty_seconds,
    a_failed_open_is_retried_with_a_growing_delay,
    a_refusal_whose_body_never_ends_is_given_up_after_three_seconds,
    a_stream_that_ends_is_reopened_after_the_last_sequence,
    a_stream_that_opened_starts_the_delay_over,
    a_connection_that_is_reset_is_retried,
    a_stream_that_breaks_is_reopened_after_the_last_sequence,
    a_permanent_status_closes_the_session,
    a_permanent_status_with_a_body_that_never_ends_closes_after_three_seconds,
    a_late_liveness_check_reopens_at_once,
    a_late_wall_clock_alone_reopens_at_once,
    a_late_monotonic_clock_alone_reopens_at_once,
    a_small_jump_is_a_lost_stream,
    a_frame_postpones_the_liveness_timer,
    a_chunk_without_a_whole_frame_does_not_postpone_it,
    a_probe_sends_a_beat_at_once,
    a_second_probe_waits_for_the_beat_spacing,
    a_third_probe_does_not_add_a_second_wait,
    a_probe_during_an_unanswered_beat_is_sent_after_it,
    closing_stops_the_liveness_timer,
    closing_stops_a_waiting_reconnect,
    closing_stops_a_stream_that_delivers,
    a_reconnect_restarts_the_heartbeat_and_opens_the_stream_after_the_last_sequence,
);

#[tokio::test(start_paused = true)]
async fn a_silent_stream_is_reopened_by_the_liveness_timer() {
    replay("a_silent_stream_is_reopened_by_the_liveness_timer").await;
}

#[tokio::test(start_paused = true)]
async fn closing_stops_a_pending_open() {
    replay("closing_stops_a_pending_open").await;
}
