//! The timers of the read stream replay the timelines the pinned upstream bundle produced with its timers, clocks and
//! `Math.random` stubbed (`stream` cases of `tests/fixtures/bridge_worker/timelines.json`): when each request for the
//! stream was sent, when the session closed, and when a probe made the heartbeat go out. The service is scripted from
//! the case, the Tokio clock is paused and the random draws are the recorded ones, so the times are exact.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::ResponseBuilderExt;
use serde_json::{Value, json};
use tokio::time::Instant;
use trogon_claude_agent_sdk::adapters::HttpExecute;
use trogon_claude_agent_sdk::{
    HeartbeatDriver, NonOrigin403, SensitiveString, StreamBody, StreamClocks, StreamConnector, StreamEffect,
    StreamFailureError, StreamRequest, StreamResponse, StreamSession, WorkerClient, WorkerConfig, WorkerPacing,
    run_stream,
};

const TIMELINES: &str = include_str!("fixtures/bridge_worker/timelines.json");
const WALL_ORIGIN: Duration = Duration::from_millis(1_700_000_000_000);

type Log = Arc<Mutex<Vec<String>>>;

fn note(log: &Log, start: Instant, line: &str) {
    log.lock()
        .unwrap()
        .push(format!("{} {line}", start.elapsed().as_millis()));
}

/// Answers every worker request `200 {}`, hangs the heartbeat answers the case scripts as `hang`, and logs the
/// heartbeats.
struct Service {
    start: Instant,
    log: Log,
    answers: Vec<Value>,
    beats: AtomicUsize,
}

impl HttpExecute for Service {
    fn execute(&self, request: reqwest::Request) -> impl Future<Output = reqwest::Result<reqwest::Response>> + Send {
        let heartbeat = request.url().path().ends_with("/worker/heartbeat");
        let hang = heartbeat && {
            note(&self.log, self.start, "request POST /worker/heartbeat");
            self.answers.get(self.beats.fetch_add(1, Ordering::SeqCst)).is_some()
        };
        let url = request.url().clone();
        async move {
            if hang {
                std::future::pending::<()>().await;
            }
            let response = http::Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .url(url)
                .body("{}".to_owned())
                .expect("a response");
            Ok(reqwest::Response::from(response))
        }
    }
}

/// Draws the recorded random values of the heartbeat schedule and waits on the Tokio timer.
struct Recorded {
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
        tokio::time::sleep(delay)
    }
}

enum Finish {
    End,
    Reset,
    Hold,
}

struct ScriptedBody {
    chunks: VecDeque<(Instant, Vec<u8>)>,
    finish: Finish,
}

impl StreamBody for ScriptedBody {
    async fn next_chunk(&mut self) -> Option<Result<Vec<u8>, StreamFailureError>> {
        if let Some((at, bytes)) = self.chunks.pop_front() {
            tokio::time::sleep_until(at).await;
            return Some(Ok(bytes));
        }
        match self.finish {
            Finish::End => None,
            Finish::Reset => Some(Err(StreamFailureError("the connection was reset".to_owned()))),
            Finish::Hold => std::future::pending().await,
        }
    }
}

/// The service of the case: the n-th request gets the n-th scripted connection, and a request past the script gets a
/// stream that stays open and silent.
struct Scripted {
    start: Instant,
    log: Log,
    conns: Vec<Value>,
    next: AtomicUsize,
}

impl StreamConnector for Scripted {
    type Body = ScriptedBody;

    fn open(
        &self,
        request: &StreamRequest,
    ) -> impl Future<Output = Result<StreamResponse<ScriptedBody>, StreamFailureError>> + Send {
        let query = request
            .query
            .as_ref()
            .map(|query| format!("?{query}"))
            .unwrap_or_default();
        note(
            &self.log,
            self.start,
            &format!("request GET /worker/events/stream{query}"),
        );
        let conn = self
            .conns
            .get(self.next.fetch_add(1, Ordering::SeqCst))
            .cloned()
            .unwrap_or_else(|| json!({}));
        async move {
            match conn.as_str() {
                Some("hang") => std::future::pending().await,
                Some(_) => Err(StreamFailureError("the connection was refused".to_owned())),
                None => {
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
                    Ok(StreamResponse {
                        status,
                        body: ScriptedBody { chunks, finish },
                    })
                }
            }
        }
    }
}

fn draws(case: &Value, key: &str) -> VecDeque<f64> {
    case[key]
        .as_array()
        .expect("randoms")
        .iter()
        .map(|value| value.as_f64().expect("a number"))
        .collect()
}

fn worker(service: Service, randoms: VecDeque<f64>, interval: Option<u64>) -> Arc<WorkerClient<Service, Recorded>> {
    Arc::new(
        WorkerClient::new(
            service,
            Recorded {
                randoms: Mutex::new(randoms),
            },
            WorkerConfig {
                api_base_url: "http://service.invalid".to_owned(),
                session_id: "cse_abc".to_owned(),
                token: SensitiveString::from("tok"),
                entrypoint: None,
                heartbeat_interval_ms: interval,
                session_access_token: None,
                non_origin_403: NonOrigin403::default(),
            },
        )
        .expect("a worker"),
    )
}

fn recorded_case(name: &str) -> Value {
    let all: Value = serde_json::from_str(TIMELINES).expect("timelines parse");
    all["timelines"]
        .as_array()
        .and_then(|cases| cases.iter().find(|case| case["name"] == name))
        .cloned()
        .expect("a recorded case")
}

async fn replay(name: &str) {
    let case = recorded_case(name);
    let script = &case["script"];
    let until = script["until"].as_u64().expect("until");
    let start = Instant::now();
    let log: Log = Arc::default();
    let client = worker(
        Service {
            start,
            log: Arc::clone(&log),
            answers: script["answers"].as_array().cloned().unwrap_or_default(),
            beats: AtomicUsize::new(0),
        },
        draws(&case, "randoms"),
        script["interval"].as_u64(),
    );
    client.initialize(2).await.expect("registered");
    let heartbeat = Arc::new(HeartbeatDriver::start(
        Arc::clone(&client),
        Arc::new(AtomicBool::new(false)),
        |_| {},
    ));

    let wall_skew = Arc::new(AtomicU64::new(0));
    let monotonic_skew = Arc::new(AtomicU64::new(0));
    let wall = Arc::clone(&wall_skew);
    let monotonic = Arc::clone(&monotonic_skew);
    let clocks = StreamClocks {
        wall: Arc::new(move || WALL_ORIGIN + start.elapsed() + Duration::from_millis(wall.load(Ordering::SeqCst))),
        monotonic: Arc::new(move || start.elapsed() + Duration::from_millis(monotonic.load(Ordering::SeqCst))),
    };
    let skews = script["skews"].as_array().cloned().unwrap_or_default();
    let jumper = tokio::spawn(async move {
        for skew in skews {
            let at = skew[0].as_u64().expect("a time");
            tokio::time::sleep_until(start + Duration::from_millis(at)).await;
            wall_skew.fetch_add(skew[1].as_u64().expect("wall"), Ordering::SeqCst);
            monotonic_skew.fetch_add(skew[2].as_u64().expect("monotonic"), Ordering::SeqCst);
        }
    });

    let connector = Scripted {
        start,
        log: Arc::clone(&log),
        conns: script["conns"].as_array().cloned().unwrap_or_default(),
        next: AtomicUsize::new(0),
    };
    let mut stream_draws = draws(&case, "stream_randoms");
    let probes = Arc::clone(&heartbeat);
    let closing = Arc::clone(&log);
    let stream = tokio::spawn(async move {
        let session = std::sync::Mutex::new(StreamSession::seeded(None));
        let code = run_stream(
            &connector,
            &session,
            &clocks,
            &mut || {
                stream_draws
                    .pop_front()
                    .expect("the stream drew more random values than the bundle did")
            },
            &mut |effect| {
                if effect == StreamEffect::HeartbeatProbe {
                    probes.probe();
                }
            },
        )
        .await;
        note(&closing, start, &format!("closed {code}"));
    });

    tokio::time::sleep_until(start + Duration::from_millis(until + 1)).await;
    stream.abort();
    jumper.abort();
    drop(heartbeat);
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
    a_silent_stream_is_reopened_by_the_liveness_timer,
    a_frame_postpones_the_liveness_timer,
    a_chunk_without_a_whole_frame_does_not_postpone_it,
    a_late_liveness_check_reopens_at_once,
    a_late_wall_clock_alone_reopens_at_once,
    a_late_monotonic_clock_alone_reopens_at_once,
    a_small_jump_is_a_lost_stream,
    a_probe_sends_a_beat_at_once,
    a_second_probe_waits_for_the_beat_spacing,
    a_third_probe_does_not_add_a_second_wait,
    a_probe_during_an_unanswered_beat_is_sent_after_it,
);

/// Not a recorded behaviour: a probe after the session closed sends nothing, and dropping the driver cancels the
/// probe's waiting timer instead of leaving it to send a beat for a session that is gone.
#[tokio::test(start_paused = true)]
async fn a_closed_session_and_a_dropped_driver_send_no_probe_beat() {
    let start = Instant::now();
    let log: Log = Arc::default();
    let client = worker(
        Service {
            start,
            log: Arc::clone(&log),
            answers: Vec::new(),
            beats: AtomicUsize::new(0),
        },
        VecDeque::from([0.5, 0.5]),
        Some(600_000),
    );
    client.initialize(2).await.expect("registered");
    let closed = Arc::new(AtomicBool::new(false));
    let driver = HeartbeatDriver::start(Arc::clone(&client), Arc::clone(&closed), |_| {});
    driver.probe();
    driver.probe();
    tokio::time::sleep(Duration::from_millis(10)).await;
    drop(driver);
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(
        vec!["0 request POST /worker/heartbeat".to_owned()],
        log.lock().unwrap().clone()
    );

    let driver = HeartbeatDriver::start(client, Arc::clone(&closed), |_| {});
    closed.store(true, Ordering::Release);
    driver.probe();
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(1, log.lock().unwrap().len());
}

/// The default clocks read the system clock and the Tokio clock.
#[tokio::test(start_paused = true)]
async fn the_default_clocks_follow_the_system_and_the_tokio_clock() {
    let clocks = StreamClocks::default();
    let before = (clocks.monotonic)();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(Duration::from_secs(5), (clocks.monotonic)() - before);
    assert!((clocks.wall)() > WALL_ORIGIN);
}
