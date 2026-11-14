//! The stream event window of an attached session replays the cases the pinned upstream bundle produced
//! (`tests/fixtures/bridge_worker/timelines.json`): what each events request carried and when it was sent, as the
//! handle is written to, flushed, closed and reconnected. The service is in memory and the Tokio clock is paused, so
//! the times are exact. Same-millisecond order between a state PUT and an events POST is not compared. A case that
//! is recorded `together` calls the handle several times in one millisecond without yielding, so the rows share one
//! turn of the upload task; the others let it finish its posts between two calls, as the recorder does.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::ResponseBuilderExt;
use serde_json::{Value, json};
use tokio::time::Instant;
use trogon_claude_agent_sdk::adapters::HttpExecute;
use trogon_claude_agent_sdk::{AttachBridgeSessionOptions, ReconnectOptions, WorkerPacing, attach_bridge_session};

const TIMELINES: &str = include_str!("fixtures/bridge_worker/timelines.json");

/// One request the service saw: when, what, and for the events route the events it carried.
type Seen = (u128, String, Option<Value>);
type Log = Arc<Mutex<Vec<Seen>>>;

#[derive(Clone)]
struct Service {
    start: Instant,
    log: Log,
}

impl HttpExecute for Service {
    fn execute(&self, request: reqwest::Request) -> impl Future<Output = reqwest::Result<reqwest::Response>> + Send {
        let route = request
            .url()
            .path()
            .trim_start_matches("/v1/code/sessions/cse_abc")
            .to_owned();
        let method = request.method().to_string();
        let body: Option<Value> = request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .and_then(|bytes| serde_json::from_slice(bytes).ok());
        let carried = (route == "/worker/events").then(|| {
            let events = body.as_ref().and_then(|body| body["events"].as_array()).cloned();
            Value::Array(
                events
                    .unwrap_or_default()
                    .into_iter()
                    .map(|event| {
                        let mut payload = event["payload"].clone();
                        if let Some(payload) = payload.as_object_mut() {
                            payload.remove("session_id");
                        }
                        let mut shown = json!({ "p": payload });
                        if event["ephemeral"] == true {
                            shown["e"] = json!(1);
                        }
                        shown
                    })
                    .collect(),
            )
        });
        if matches!(route.as_str(), "/worker" | "/worker/events") && method != "GET" {
            self.log
                .lock()
                .unwrap()
                .push((self.start.elapsed().as_millis(), format!("{method} {route}"), carried));
        }
        let url = request.url().clone();
        async move {
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

#[derive(Clone)]
struct Instant0;

impl WorkerPacing for Instant0 {
    fn jitter(&self) -> f64 {
        0.0
    }

    async fn pause(&self, _delay: Duration) {}
}

/// The recorded line `<ms> request <METHOD> <route>[ <json>]` as what the service saw.
fn parsed(line: &str) -> Seen {
    let (millis, rest) = line.split_once(" request ").expect("a request line");
    let mut parts = rest.splitn(3, ' ');
    let method = parts.next().expect("a method");
    let route = parts.next().expect("a route");
    let carried = parts
        .next()
        .map(|json| serde_json::from_str(json).expect("events json"));
    (millis.parse().expect("a time"), format!("{method} {route}"), carried)
}

async fn until(start: Instant, millis: u64) {
    tokio::time::sleep_until(start + Duration::from_millis(millis)).await;
}

async fn replay(name: &str) {
    let all: Value = serde_json::from_str(TIMELINES).expect("timelines parse");
    let case = all["timelines"]
        .as_array()
        .and_then(|cases| cases.iter().find(|case| case["name"] == name))
        .expect("a recorded case");
    let script = &case["script"];
    let start = Instant::now();
    let log: Log = Arc::default();
    let mut options = AttachBridgeSessionOptions::new("cse_abc", "http://service.invalid", "tok");
    options.epoch = Some(2);
    options.outbound_only = true;
    options.arguments = Some(Vec::new());
    options.caller_driven = true;
    options.environment = Some(Arc::new(|_| None));
    let mut session = attach_bridge_session(
        Service {
            start,
            log: Arc::clone(&log),
        },
        Instant0,
        options,
    )
    .await
    .expect("attached");
    session.spawn_uploads();
    let mut steps: Vec<(u64, Value)> = script["writes"]
        .as_array()
        .expect("writes")
        .iter()
        .map(|write| (write[0].as_u64().expect("a time"), write[1].clone()))
        .collect();
    for key in ["flush_at", "close_at", "reconnect_at"] {
        if let Some(at) = script[key].as_u64() {
            steps.push((at, json!(key)));
        }
    }
    steps.sort_by_key(|(at, _)| *at);
    let together = script["together"] == true;
    let mut last = None;
    for (at, step) in steps {
        if last != Some(at) {
            until(start, at).await;
        } else if !together {
            session.uploaded().await;
        }
        last = Some(at);
        match step.as_str() {
            Some("flush_at") => session.flush().await,
            Some("close_at") => session.close().await,
            Some(_) => {
                let reconnect = ReconnectOptions {
                    api_base_url: "http://service.invalid".to_owned(),
                    ingress_token: "tok2".into(),
                    epoch: None,
                };
                session.reconnect_transport(reconnect).await.expect("reconnected");
            }
            None => session.write(step.as_object().cloned().expect("a frame")),
        }
    }
    until(start, script["until"].as_u64().expect("until") + 1).await;
    let recorded: Vec<Seen> = case["timeline"]
        .as_array()
        .expect("a timeline")
        .iter()
        .filter_map(Value::as_str)
        .filter(|line| line.contains(" request "))
        .map(parsed)
        .filter(|(_, route, _)| route != "POST /worker/heartbeat")
        .collect();
    let seen = log.lock().unwrap().clone();
    for events in [true, false] {
        let pick = |all: &[Seen]| -> Vec<Seen> {
            all.iter()
                .filter(|(_, route, _)| route.contains("events") == events)
                .cloned()
                .collect()
        };
        assert_eq!(pick(&seen), pick(&recorded), "{name} (events: {events})");
    }
}

macro_rules! replayed {
    ($($name:ident),+ $(,)?) => {
        $(
            #[tokio::test(start_paused = true)]
            async fn $name() {
                replay(stringify!($name)).await;
            }
        )+
    };
}

replayed!(
    deltas_in_one_window_are_sent_together_when_it_ends,
    a_row_that_is_not_a_stream_event_sends_the_buffered_deltas_ahead_of_it,
    an_assistant_text_row_waits_for_the_window_to_end,
    an_assistant_row_with_a_tool_call_does_not_wait,
    an_assistant_text_row_with_nothing_buffered_is_sent_at_once,
    an_assistant_row_without_content_does_not_wait,
    a_block_start_sends_the_window_when_a_row_waits,
    a_block_start_does_not_send_the_window_when_no_row_waits,
    waiting_rows_follow_the_deltas_when_the_window_ends,
    deltas_of_another_kind_block_or_parent_are_not_merged,
    deltas_that_carry_no_text_are_never_merged,
    thinking_deltas_add_up_their_token_estimates,
    a_lone_delta_is_sent_as_it_came,
    the_transformations_of_a_message_are_not_sent,
    a_delta_over_the_size_limit_is_dropped,
    big_deltas_that_do_not_fit_together_stay_apart,
    multibyte_deltas_are_measured_in_bytes,
    escaped_characters_count_as_their_escapes,
    flushing_sends_the_window_at_once,
    closing_drops_the_window,
    closing_sends_the_row_that_waits_behind_the_ready_one,
    reconnecting_sends_the_waiting_rows_and_drops_the_deltas,
    two_rows_written_together_are_posted_apart,
    several_rows_written_together_follow_the_first_in_one_post,
    rows_written_together_behind_a_window_follow_the_window_row_in_one_post,
    two_rows_written_together_behind_a_window_are_posted_apart,
    rows_written_together_in_two_milliseconds_are_posted_by_millisecond,
    rows_written_together_follow_the_first_in_posts_of_a_hundred,
    rows_written_together_and_a_close_in_the_same_step,
    rows_written_together_behind_a_window_and_a_close_in_the_same_step,
    rows_and_a_flush_in_the_same_step,
);

#[tokio::test(start_paused = true)]
async fn closing_sends_the_row_that_is_ready() {
    replay("closing_sends_the_row_that_is_ready").await;
}
