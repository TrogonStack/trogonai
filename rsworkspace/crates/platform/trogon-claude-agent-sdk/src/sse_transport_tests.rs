//! Replays `tests/fixtures/sse_transport/cases.jsonl`, recorded by running the browser SDK's SSE transport class
//! against a local HTTP server, over a hand-written HTTP/1.1 server. Every wait is divided by the recorded divisor.
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests exercise exact upstream JSON fixtures, including malformed and extension shapes."
    )
)]

use std::{
    error::Error as StdError,
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::{Map, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use super::*;

type TestResult = Result<(), Box<dyn StdError>>;

const FIXTURE: &str = include_str!("../tests/fixtures/sse_transport/cases.jsonl");
const PLACEHOLDER_UUID: &str = "00000000-0000-4000-8000-000000000001";
const GET_HEADERS: [&str; 5] = ["accept", "last-event-id", "authorization", "x-custom", "x-dup"];
const POST_HEADERS: [&str; 4] = ["content-type", "authorization", "x-custom", "x-dup"];

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn golden(name: &str) -> Result<Value, Box<dyn StdError>> {
    for line in FIXTURE.lines() {
        let case: Value = serde_json::from_str(line)?;
        if case["name"] == name {
            return Ok(case);
        }
    }
    Err(format!("missing golden {name}").into())
}

fn frame(event: &str, data: &str) -> String {
    format!("event: {event}\ndata: {data}\n\n")
}

fn hold() -> Value {
    json!({"chunks": [], "end": "hold"})
}

async fn until(mut condition: impl FnMut() -> bool) -> bool {
    for _ in 0..600 {
        if condition() {
            return true;
        }
        sleep(Duration::from_millis(5)).await;
    }
    false
}

async fn closed_origin() -> Result<String, Box<dyn StdError>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    Ok(format!("http://{}", listener.local_addr()?))
}

fn sse_error(error: Error) -> Result<SseError, Box<dyn StdError>> {
    match error {
        Error::Transport(inner) => inner
            .downcast::<SseError>()
            .map(|error| *error)
            .map_err(|other| Box::<dyn StdError>::from(format!("not an SseError: {other}"))),
        other => Err(format!("not a transport error: {other:?}").into()),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The server

#[derive(Default)]
struct Seen {
    gets: Vec<Value>,
    posts: Vec<Value>,
}

struct Server {
    origin: String,
    seen: Arc<Mutex<Seen>>,
}

impl Server {
    fn gets(&self) -> usize {
        locked(&self.seen).gets.len()
    }

    fn posts(&self) -> usize {
        locked(&self.seen).posts.len()
    }
}

/// Starts a server that answers GETs from `gets` by connection number (the last entry repeats) and POSTs from
/// `posts` (200 once they run out). See `sse_transport_goldens.mjs` for the script shapes; `stall` answers with the
/// head of a response whose body never comes.
async fn serve(gets: Value, posts: Value) -> Result<Server, Box<dyn StdError>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let seen = Arc::new(Mutex::new(Seen::default()));
    let shared = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            tokio::spawn(connection(socket, gets.clone(), posts.clone(), Arc::clone(&shared)));
        }
    });
    Ok(Server { origin, seen })
}

struct Request {
    method: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

async fn read_request(socket: &mut TcpStream, buffer: &mut Vec<u8>) -> Option<Request> {
    let end = loop {
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        let mut chunk = [0u8; 4096];
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(chunk.get(..read)?);
    };
    let head = String::from_utf8_lossy(buffer.get(..end)?).into_owned();
    buffer.drain(..end + 4);
    let mut lines = head.lines();
    let method = lines.next()?.split_whitespace().next()?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    while buffer.len() < length {
        let mut chunk = [0u8; 4096];
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(chunk.get(..read)?);
    }
    let body = buffer.drain(..length).collect();
    Some(Request { method, headers, body })
}

async fn connection(mut socket: TcpStream, gets: Value, posts: Value, seen: Arc<Mutex<Seen>>) {
    let mut buffer = Vec::new();
    while let Some(request) = read_request(&mut socket, &mut buffer).await {
        let is_post = request.method == "POST";
        let wanted: &[&str] = if is_post { &POST_HEADERS } else { &GET_HEADERS };
        let mut picked = Map::new();
        for name in wanted {
            if let Some((_, value)) = request.headers.iter().find(|(header, _)| header == name) {
                picked.insert((*name).to_owned(), json!(value));
            }
        }
        let entry = if is_post {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            json!({"headers": picked, "body": body})
        } else {
            json!({"headers": picked})
        };
        let index = {
            let mut seen = locked(&seen);
            let list = if is_post { &mut seen.posts } else { &mut seen.gets };
            list.push(entry);
            list.len() - 1
        };
        let script = if is_post {
            posts.get(index).cloned().unwrap_or_else(|| json!({"status": 200}))
        } else {
            let last = gets.as_array().map_or(0, |list| list.len().saturating_sub(1));
            gets.get(index.min(last)).cloned().unwrap_or(Value::Null)
        };
        if script.is_null() || script["end"] == "hang" {
            pending::<()>().await;
        }
        if !respond(&mut socket, &script, is_post).await {
            return;
        }
    }
}

/// Writes the scripted answer; false when the connection cannot be used for another request.
async fn respond(socket: &mut TcpStream, script: &Value, is_post: bool) -> bool {
    let status = script["status"].as_u64().unwrap_or(200);
    let extra: String = script["headers"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, value)| value.as_str().map(|value| format!("{name}: {value}\r\n")))
        .collect();
    if script["stall"] == true {
        let head = format!("HTTP/1.1 {status} X\r\n{extra}content-length: 1000\r\n\r\n");
        let _ = socket.write_all(head.as_bytes()).await;
        pending::<()>().await;
    }
    if is_post || script.get("chunks").is_none() {
        let body = script["body"].as_str().unwrap_or_default();
        let answer = format!(
            "HTTP/1.1 {status} X\r\n{extra}content-length: {}\r\n\r\n{body}",
            body.len()
        );
        return socket.write_all(answer.as_bytes()).await.is_ok();
    }
    let head =
        format!("HTTP/1.1 {status} X\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n{extra}\r\n");
    if socket.write_all(head.as_bytes()).await.is_err() {
        return false;
    }
    for chunk in script["chunks"].as_array().into_iter().flatten() {
        if let Some(text) = chunk.as_str() {
            let encoded = format!("{:x}\r\n{text}\r\n", text.len());
            if socket.write_all(encoded.as_bytes()).await.is_err() {
                return false;
            }
        }
    }
    match script["end"].as_str() {
        Some("close") => socket.write_all(b"0\r\n\r\n").await.is_ok(),
        Some("reset") => {
            sleep(Duration::from_millis(60)).await;
            false
        }
        _ => {
            pending::<()>().await;
            false
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Replaying a recorded case

struct Replay {
    golden: Value,
    results: Value,
    gets: Value,
    posts: Value,
}

fn ccr_json(event: Option<SseCcrEvent>) -> Value {
    event.map_or(Value::Null, |event| {
        let mut value = json!({"event_id": event.event_id, "sequence_num": event.sequence_num});
        if let Some(created_at) = event.created_at {
            value["created_at"] = json!(created_at);
        }
        if let Some(source) = event.source {
            value["source"] = json!(source);
        }
        value
    })
}

fn failing_handler(throwing: bool) {
    if throwing {
        std::panic::resume_unwind(Box::new("handler failed"));
    }
}

fn hosted(value: &Value, origin: &str) -> Result<Value, Box<dyn StdError>> {
    Ok(serde_json::from_str(
        &serde_json::to_string(value)?.replace(origin, "HOST"),
    )?)
}

fn without_uuids(posts: &mut Value) {
    for post in posts.as_array_mut().into_iter().flatten() {
        if let Some(uuid) = post.pointer_mut("/body/events/0/payload/uuid")
            && uuid.as_str().is_some_and(|text| uuid::Uuid::parse_str(text).is_ok())
        {
            *uuid = json!(PLACEHOLDER_UUID);
        }
    }
}

async fn replay(name: &str) -> Result<Replay, Box<dyn StdError>> {
    let golden = golden(name)?;
    let spec = &golden["spec"];
    let server = serve(spec["gets"].clone(), spec["posts"].clone()).await?;
    let dead = closed_origin().await?;
    let settings = &spec["options"];
    let stream_url = match settings["streamUrl"].as_str() {
        Some("closed") => format!("{dead}/stream"),
        Some(url) => url.to_owned(),
        None => format!("{}/stream", server.origin),
    };
    let send_url = if settings["sendUrl"] == "closed" {
        format!("{dead}/send")
    } else {
        format!("{}/send", server.origin)
    };
    let mut options = SseOptions::new(stream_url, send_url, "session-1");
    for (name, value) in settings["headers"].as_object().into_iter().flatten() {
        options
            .headers
            .insert(name.clone(), value.as_str().unwrap_or_default().to_owned());
    }
    options.from_sequence_num = settings["fromSequenceNum"].as_f64();
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let throwing = settings["throwing"] == true;
    let truncated_events = Arc::clone(&events);
    options.on_catch_up_truncated = Some(Arc::new(move || {
        locked(&truncated_events).push(json!(["catch_up_truncated"]));
        failing_handler(throwing);
    }));
    let update_events = Arc::clone(&events);
    options.on_delivery_update = Some(Arc::new(move |update: SseDeliveryUpdate| {
        let mut value = json!({"event_id": update.event_id, "status": update.status});
        if let Some(timestamp) = update.timestamp {
            value["timestamp"] = json!(timestamp);
        }
        locked(&update_events).push(json!(["delivery_update", value]));
        failing_handler(throwing);
    }));
    let abort = CancellationToken::new();
    if settings["abortFirst"] == true {
        abort.cancel();
    }
    let divisor = u32::try_from(golden["divisor"].as_u64().unwrap_or(1))?;
    let mut transport = SseTransport::new(options)
        .with_abort(abort.clone())
        .with_time_divisor(divisor);
    let handle = transport.handle();
    transport.connect().await?;
    let mut results = Vec::new();
    for action in spec["actions"].as_array().into_iter().flatten() {
        let name = action[0].as_str().unwrap_or_default();
        let argument = &action[1];
        let result = match name {
            "read" => match timeout(Duration::from_secs(2), transport.read_message()).await {
                Err(_) => json!({"timeout": true}),
                Ok(Ok(Some(message))) => {
                    let ccr = ccr_json(handle.ccr_event(&message));
                    handle.mark_delivered(&message);
                    json!({"message": message, "ccr": ccr})
                }
                Ok(Ok(None)) => json!({"done": true}),
                Ok(Err(error)) => json!({"error": error.to_string()}),
            },
            "write" => {
                let line = argument.as_str().unwrap_or_default();
                match timeout(Duration::from_secs(2), transport.write(line)).await {
                    Err(_) => json!({"error": "write timeout"}),
                    Ok(Ok(())) => json!({"ok": true}),
                    Ok(Err(error)) => json!({"error": error.to_string()}),
                }
            }
            "writeMany" => {
                let resolved = Arc::new(AtomicUsize::new(0));
                for n in 0..argument.as_u64().unwrap_or_default() {
                    let shared = Arc::clone(&transport.shared);
                    let resolved = Arc::clone(&resolved);
                    let line = json!({"type": "user", "n": n}).to_string();
                    tokio::spawn(async move {
                        if shared.write(&line).await.is_ok() {
                            resolved.fetch_add(1, Ordering::SeqCst);
                        }
                    });
                }
                sleep(Duration::from_millis(300)).await;
                json!({"resolved": resolved.load(Ordering::SeqCst)})
            }
            "waitGets" => {
                let wanted = usize::try_from(argument.as_u64().unwrap_or_default())?;
                json!({"reached": until(|| server.gets() >= wanted).await})
            }
            "waitPosts" => {
                let wanted = usize::try_from(argument.as_u64().unwrap_or_default())?;
                json!({"reached": until(|| server.posts() >= wanted).await})
            }
            "waitReady" => json!({"reached": until(|| transport.is_ready()).await}),
            "settle" => {
                sleep(Duration::from_millis(argument.as_u64().unwrap_or_default())).await;
                json!({"settled": true})
            }
            "suspend" => json!({"suspended": handle.suspend()}),
            "resume" => json!({"resumed": handle.resume()}),
            "expect" => {
                handle.expect_control_response(argument.as_str().unwrap_or_default());
                json!({"expected": true})
            }
            "abort" => {
                abort.cancel();
                sleep(Duration::from_millis(20)).await;
                json!({"aborted": true})
            }
            "close" => {
                transport.close().await?;
                sleep(Duration::from_millis(10)).await;
                json!({"closed": true})
            }
            "snapshot" => {
                let drops: Map<String, Value> = handle
                    .drop_counts()
                    .into_iter()
                    .map(|(category, count)| (category.to_owned(), json!(count)))
                    .collect();
                json!({
                    "ready": transport.is_ready(),
                    "drops": drops,
                    "last": handle.last_sequence_num(),
                    "events": std::mem::take(&mut *locked(&events)),
                })
            }
            other => return Err(format!("unknown action {other}").into()),
        };
        results.push(json!([name, result]));
    }
    let (gets, mut posts) = {
        let seen = locked(&server.seen);
        (Value::Array(seen.gets.clone()), Value::Array(seen.posts.clone()))
    };
    without_uuids(&mut posts);
    let origin = server.origin.as_str();
    Ok(Replay {
        results: hosted(&Value::Array(results), origin)?,
        gets: hosted(&gets, origin)?,
        posts: hosted(&posts, origin)?,
        golden,
    })
}

fn without_logs(results: &mut Value) {
    for pair in results.as_array_mut().into_iter().flatten() {
        if let Some(result) = pair.get_mut(1).and_then(Value::as_object_mut) {
            result.remove("logs");
        }
    }
}

async fn replays(name: &str) -> TestResult {
    let replay = replay(name).await?;
    let mut expected = replay.golden["results"].clone();
    without_logs(&mut expected);
    assert_eq!(replay.results, expected, "{name}: results");
    assert_eq!(replay.gets, replay.golden["gets"], "{name}: stream requests");
    assert_eq!(replay.posts, replay.golden["posts"], "{name}: writes");
    Ok(())
}

macro_rules! replayed {
    ($($test:ident => $case:literal,)*) => {
        const REPLAYED: &[&str] = &[$($case),*];
        $(
            #[tokio::test]
            async fn $test() -> TestResult {
                replays($case).await
            }
        )*
    };
}

replayed! {
    a_session_matches_the_recorded_transport => "session",
    handlers_that_throw_do_not_stop_the_stream => "delivery_handlers_throw",
    a_401_on_the_stream_is_permanent => "stream_permanent_401",
    a_403_on_the_stream_is_permanent => "stream_permanent_403",
    a_404_on_the_stream_is_permanent => "stream_permanent_404",
    a_permanent_stream_error_without_a_reason => "stream_permanent_without_reason",
    a_server_error_on_the_stream_is_retried => "stream_retries_after_a_server_error",
    the_reconnect_budget_runs_out => "reconnect_budget_exhausted",
    a_reset_stream_reconnects_from_the_cursor => "stream_reset_reconnects_from_the_cursor",
    an_ended_stream_reconnects => "stream_end_reconnects",
    a_silent_stream_is_reconnected => "liveness_timeout_reconnects",
    a_stream_that_never_answers_is_reconnected => "connect_timeout_reconnects",
    suspending_and_resuming_the_stream => "suspend_and_resume",
    a_write_resumes_a_suspended_stream => "a_write_resumes_a_suspended_stream",
    expecting_a_response_resumes_a_suspended_stream => "expecting_a_response_resumes_a_suspended_stream",
    a_failed_write_waits_for_retry_after => "post_waits_for_retry_after",
    a_write_is_dropped_after_three_failures => "post_is_dropped_after_three_failures",
    a_permanent_write_failure_fails_the_transport => "post_permanent_failure_fails_the_transport",
    a_permanent_write_failure_without_a_reason => "post_permanent_failure_without_reason",
    a_write_that_never_answers_times_out => "post_that_never_answers_times_out",
    a_write_to_an_unreachable_server_is_retried => "post_to_an_unreachable_server_is_retried",
    aborting_before_connecting => "abort_before_connecting",
    aborting_while_running => "abort_while_running",
    closing_before_connecting => "close_before_connecting",
    writing_after_closing => "write_after_close",
    a_stream_url_that_does_not_parse => "stream_url_that_does_not_parse",
    a_stream_server_that_is_unreachable => "stream_server_that_is_unreachable",
    a_header_name_that_is_invalid => "header_name_that_is_invalid",
    a_header_value_that_is_invalid => "header_value_that_is_invalid",
    the_queue_holds_a_thousand_lines => "the_queue_holds_a_thousand_lines",
    closing_ends_a_read => "the_stream_closing_ends_a_read",
}

#[test]
fn every_recorded_case_is_replayed() -> TestResult {
    let mut recorded = Vec::new();
    for line in FIXTURE.lines() {
        let case: Value = serde_json::from_str(line)?;
        recorded.push(case["name"].as_str().unwrap_or_default().to_owned());
    }
    assert_eq!(recorded, REPLAYED);
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// The recorded waits

fn waits(name: &str, kind: &str) -> Result<Vec<u64>, Box<dyn StdError>> {
    Ok(golden(name)?["delays"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|delay| delay[0] == kind)
        .filter_map(|delay| delay[1].as_u64())
        .collect())
}

#[test]
fn the_recorded_waits_are_the_ones_the_transport_uses() -> TestResult {
    let reconnects: Vec<u64> = (1..=5).map(|attempt| millis(reconnect_delay(attempt))).collect();
    assert_eq!(waits("reconnect_budget_exhausted", "reconnect")?, reconnects);
    assert_eq!(reconnects, [1_000, 2_000, 4_000, 8_000, 16_000]);
    assert_eq!(waits("stream_url_that_does_not_parse", "reconnect")?, reconnects);
    let connect = waits("connect_timeout_reconnects", "connect")?;
    assert_eq!(connect, [millis(CONNECT_TIMEOUT); 2]);
    let liveness = waits("liveness_timeout_reconnects", "liveness")?;
    assert!(!liveness.is_empty() && liveness.iter().all(|wait| *wait == millis(LIVENESS_TIMEOUT)));
    let posts = waits("post_is_dropped_after_three_failures", "post")?;
    assert_eq!(posts, [millis(POST_TIMEOUT); 4]);
    assert_eq!(RECONNECT_BUDGET, 5);
    Ok(())
}

#[test]
fn the_recorded_write_retries_are_the_ones_the_transport_uses() -> TestResult {
    let after_headers: Vec<u64> = [Some("2"), Some("90"), Some("0"), Some("soon")]
        .into_iter()
        .map(|header| millis(retry_delay(1, retry_after_ms(header), Duration::ZERO)))
        .collect();
    assert_eq!(waits("post_waits_for_retry_after", "other")?, after_headers);
    let backoff: Vec<u64> = (1..=2)
        .map(|failures| millis(retry_delay(failures, None, Duration::ZERO)))
        .collect();
    assert_eq!(waits("post_is_dropped_after_three_failures", "other")?, backoff);
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// What the recorded cases do not reach

fn transport_for(server: &Server, divisor: u32) -> SseTransport {
    SseTransport::new(SseOptions::new(
        format!("{}/stream", server.origin),
        format!("{}/send", server.origin),
        "session-1",
    ))
    .with_time_divisor(divisor)
}

#[test]
fn the_text_envelope_arm_is_ported() {
    assert!(std::hint::black_box(G4_TEXT_ENVELOPE_ARM_PORTED));
}

#[tokio::test]
async fn a_written_line_that_is_not_json_is_refused() -> TestResult {
    let server = serve(json!([hold()]), json!([])).await?;
    let mut transport = transport_for(&server, 1);
    transport.connect().await?;
    let error = sse_error(transport.write("not json").await.err().ok_or("the write succeeded")?)?;
    assert!(matches!(error, SseError::InvalidJson(_)), "{error:?}");
    assert_eq!(server.posts(), 0);
    Ok(())
}

#[tokio::test]
async fn connecting_twice_changes_nothing_and_ending_input_is_allowed() -> TestResult {
    let server = serve(json!([hold()]), json!([])).await?;
    let mut transport = transport_for(&server, 1);
    transport.connect().await?;
    transport.connect().await?;
    assert!(until(|| transport.is_ready()).await);
    transport.end_input().await?;
    sleep(Duration::from_millis(50)).await;
    assert_eq!(server.gets(), 1);
    assert!(transport.is_ready());
    Ok(())
}

#[tokio::test]
async fn dropping_the_transport_closes_it() -> TestResult {
    let server = serve(json!([hold()]), json!([])).await?;
    let mut transport = transport_for(&server, 1);
    let handle = transport.handle();
    transport.connect().await?;
    assert!(until(|| server.gets() == 1).await);
    drop(transport);
    assert!(!handle.suspend());
    assert!(!handle.resume());
    Ok(())
}

#[tokio::test]
async fn a_write_whose_headers_cannot_be_built_fails_the_attempt() -> TestResult {
    let mut options = SseOptions::new("http://127.0.0.1:1/stream", "http://127.0.0.1:1/send", "session-1");
    options.headers.insert("bad name".to_owned(), "x".to_owned());
    let transport = SseTransport::new(options);
    assert_eq!(transport.shared.post_once("{}".to_owned()).await, Err(None));
    Ok(())
}

#[tokio::test]
async fn a_stream_cancelled_before_it_was_ready_is_not_marked_ready() {
    let transport = SseTransport::new(SseOptions::new(
        "http://127.0.0.1:1/s",
        "http://127.0.0.1:1/p",
        "session-1",
    ));
    let cancelled = transport.shared.closed.child_token();
    cancelled.cancel();
    assert!(!transport.shared.mark_ready(&cancelled));
    assert!(!transport.is_ready());
    assert_eq!(transport.shared.stream_ended(&cancelled, None), None);
    let live = transport.shared.closed.child_token();
    assert!(transport.shared.mark_ready(&live));
    assert!(transport.is_ready());
    assert_eq!(
        transport.shared.stream_ended(&live, Some(SseError::LivenessTimeout)),
        Some(reconnect_delay(1))
    );
}

#[tokio::test]
async fn frames_after_a_suspension_in_the_same_chunk_are_not_dispatched() -> TestResult {
    let both = format!(
        "{}{}",
        frame("delivery_update", r#"{"event_id":"e1","status":"sent"}"#),
        frame(
            "client_event",
            r#"{"event_id":"e2","sequence_num":2,"source":"worker","payload":{"type":"assistant","message":{"content":[]}}}"#
        )
    );
    let server = serve(json!([{"chunks": [both], "end": "hold"}, hold()]), json!([])).await?;
    let slot: Arc<Mutex<Option<SseHandle>>> = Arc::new(Mutex::new(None));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut options = SseOptions::new(
        format!("{}/stream", server.origin),
        format!("{}/send", server.origin),
        "s",
    );
    let (handler_slot, handler_calls) = (Arc::clone(&slot), Arc::clone(&calls));
    options.on_delivery_update = Some(Arc::new(move |_| {
        handler_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(handle) = locked(&handler_slot).as_ref() {
            handle.suspend();
        }
    }));
    let mut transport = SseTransport::new(options);
    *locked(&slot) = Some(transport.handle());
    transport.connect().await?;
    assert!(until(|| calls.load(Ordering::SeqCst) == 1).await);
    assert!(
        timeout(Duration::from_millis(150), transport.read_message())
            .await
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn a_resume_that_arrived_early_does_not_cut_a_reconnect_wait_short() -> TestResult {
    let server = serve(json!([hold(), {"chunks": [], "end": "close"}, hold()]), json!([])).await?;
    let mut transport = transport_for(&server, 100);
    let handle = transport.handle();
    transport.connect().await?;
    assert!(until(|| transport.is_ready()).await);
    assert!(handle.suspend());
    assert!(handle.resume());
    assert!(until(|| server.gets() == 3).await);
    Ok(())
}

#[tokio::test]
async fn suspending_while_connecting_interrupts_the_request() -> TestResult {
    let server = serve(json!([{"end": "hang"}]), json!([])).await?;
    let mut transport = transport_for(&server, 1);
    let handle = transport.handle();
    transport.connect().await?;
    assert!(until(|| server.gets() == 1).await);
    assert!(handle.suspend());
    sleep(Duration::from_millis(30)).await;
    assert_eq!(server.gets(), 1);
    assert!(handle.resume());
    assert!(until(|| server.gets() == 2).await);
    Ok(())
}

#[tokio::test]
async fn suspending_while_an_error_body_is_read_interrupts_it() -> TestResult {
    let server = serve(json!([{"status": 500, "stall": true}]), json!([])).await?;
    let mut transport = transport_for(&server, 1);
    let handle = transport.handle();
    transport.connect().await?;
    assert!(until(|| server.gets() == 1).await);
    sleep(Duration::from_millis(30)).await;
    assert!(handle.suspend());
    sleep(Duration::from_millis(30)).await;
    assert!(handle.resume());
    assert!(until(|| server.gets() == 2).await);
    Ok(())
}

#[tokio::test]
async fn an_error_body_that_stalls_is_given_up_at_the_connect_deadline() -> TestResult {
    let server = serve(json!([{"status": 500, "stall": true}]), json!([])).await?;
    let mut transport = transport_for(&server, 100);
    transport.connect().await?;
    assert!(until(|| server.gets() == 2).await);
    Ok(())
}

#[tokio::test]
async fn closing_during_a_retry_wait_stops_every_task() -> TestResult {
    let dead = closed_origin().await?;
    let server = serve(json!([hold()]), json!([])).await?;
    let options = SseOptions::new(format!("{}/stream", server.origin), format!("{dead}/send"), "session-1");
    let mut transport = SseTransport::new(options);
    transport.connect().await?;
    transport.write(r#"{"type":"user"}"#).await?;
    sleep(Duration::from_millis(150)).await;
    transport.close().await?;
    assert!(until(|| Arc::strong_count(&transport.shared) == 1).await);
    Ok(())
}

#[tokio::test]
async fn closing_a_suspended_transport_stops_every_task() -> TestResult {
    let server = serve(json!([hold()]), json!([])).await?;
    let mut transport = transport_for(&server, 1);
    let handle = transport.handle();
    transport.connect().await?;
    assert!(until(|| transport.is_ready()).await);
    assert!(handle.suspend());
    sleep(Duration::from_millis(30)).await;
    transport.close().await?;
    drop(handle);
    assert!(until(|| Arc::strong_count(&transport.shared) == 1).await);
    Ok(())
}

#[test]
fn expecting_more_responses_than_are_remembered_evicts_the_oldest() {
    let transport = SseTransport::new(SseOptions::new(
        "http://127.0.0.1:1/s",
        "http://127.0.0.1:1/p",
        "session-1",
    ));
    let handle = transport.handle();
    for n in 0..1_030 {
        handle.expect_control_response(&format!("request-{n}"));
    }
    assert!(handle.drop_counts().is_empty());
    assert_eq!(handle.last_sequence_num(), None);
    assert_eq!(handle.ccr_event(&JsonObject::new()), None);
}
