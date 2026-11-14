//! Replays the recorded read stream cases (`stream` section of the goldens, written by
//! `.config/mise/lib/claude-sdk-freshness/bridge_worker_goldens.mjs` against the real `attachBridgeSession`): the
//! scripted connections go through [`StreamSession`] and what it asks for must match what the upstream client sent
//! and reported. The lines it delivers go through [`InboundRouter`], so the user message de-duplication is the ported
//! one.

use std::time::Duration;

use serde_json::Value;
use trogon_claude_agent_sdk::{
    FrameParser, Inbound, InboundRouter, KEEPALIVE_SECONDS, LIVENESS_GRACE_MS, PERMANENT_STREAM_STATUSES,
    RECONNECT_BASE_MS, RECONNECT_CAP_MS, RECONNECT_JITTER_RATIO, Reopen, SseFrame, StreamEffect, StreamOpen,
    StreamRequest, StreamSession, liveness_timeout, reconnect_delay,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const GOLDENS: &str = include_str!("fixtures/bridge_worker/goldens.json");
const STREAM_PATH: &str = "/worker/events/stream";

fn goldens() -> Result<Value, serde_json::Error> {
    serde_json::from_str(GOLDENS)
}

fn text(value: &Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("missing {key} in {value}"))
}

fn number(value: &Value, key: &str) -> Result<u64, String> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("missing {key} in {value}"))
}

fn list<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("missing {key} in {value}"))
}

fn chunk_bytes(chunk: &Value) -> Result<Vec<u8>, String> {
    if let Some(text) = chunk.as_str() {
        return Ok(text.as_bytes().to_vec());
    }
    list(chunk, "bytes")?
        .iter()
        .map(|byte| {
            byte.as_u64()
                .and_then(|byte| u8::try_from(byte).ok())
                .ok_or_else(|| format!("bad byte in {chunk}"))
        })
        .collect()
}

fn optional(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn milliseconds(delay: Duration) -> u64 {
    (delay.as_secs_f64() * 1000.0).round() as u64
}

#[test]
fn the_constants_are_the_recorded_ones() -> TestResult {
    let goldens = goldens()?;
    let pure = &goldens["pure"]["stream"];
    assert_eq!(RECONNECT_BASE_MS, number(pure, "reconnect_base_ms")?);
    assert_eq!(RECONNECT_CAP_MS, number(pure, "reconnect_cap_ms")?);
    assert_eq!(KEEPALIVE_SECONDS, number(pure, "keepalive_seconds")?);
    assert_eq!(LIVENESS_GRACE_MS, number(pure, "liveness_grace_ms")?);
    assert_eq!(
        liveness_timeout(),
        Duration::from_millis(number(pure, "keepalive_seconds")? * 1000 + number(pure, "liveness_grace_ms")?)
    );
    let statuses: Vec<u64> = list(pure, "permanent_statuses")?
        .iter()
        .filter_map(Value::as_u64)
        .collect();
    let ported: Vec<u64> = PERMANENT_STREAM_STATUSES
        .iter()
        .map(|status| u64::from(*status))
        .collect();
    assert_eq!(ported, statuses);
    assert!((RECONNECT_JITTER_RATIO - 0.25).abs() < f64::EPSILON);
    Ok(())
}

#[test]
fn the_reconnect_delay_grows_to_a_cap_with_jitter() -> TestResult {
    let goldens = goldens()?;
    let rows = list(&goldens["pure"]["stream"], "backoff")?;
    assert_eq!(8, rows.len());
    for row in rows {
        let row = row.as_array().ok_or("a backoff row is a list")?;
        let attempt = u32::try_from(row[0].as_u64().ok_or("attempt")?)?;
        for (position, random) in [(1, 0.0), (2, 0.5), (3, 1.0)] {
            let expected = row[position].as_u64().ok_or("delay")?;
            assert_eq!(
                expected,
                milliseconds(reconnect_delay(attempt, random)),
                "attempt {attempt} with draw {random}"
            );
        }
    }
    Ok(())
}

#[test]
fn frames_are_cut_the_way_the_upstream_parser_cuts_them() -> TestResult {
    let goldens = goldens()?;
    let cases = list(&goldens["pure"]["stream"], "frames")?;
    assert!(cases.len() >= 30, "only {} frame cases", cases.len());
    for case in cases {
        let mut parser = FrameParser::default();
        let mut frames = Vec::new();
        for chunk in list(case, "chunks")? {
            frames.extend(parser.push_bytes(&chunk_bytes(chunk)?));
        }
        let expected: Vec<SseFrame> = list(case, "frames")?
            .iter()
            .map(|frame| SseFrame {
                id: optional(frame, "id"),
                event: optional(frame, "event"),
                data: optional(frame, "data"),
            })
            .collect();
        assert_eq!(expected, frames, "frames of {}", case["chunks"]);
        assert_eq!(
            text(case, "pending")?,
            parser.buffered(),
            "pending of {}",
            case["chunks"]
        );
    }
    Ok(())
}

#[derive(Debug, Default, PartialEq)]
struct Observed {
    gets: Vec<StreamRequest>,
    delays: Vec<u64>,
    closes: Vec<u16>,
    acks: Vec<(String, String)>,
    inbound: Vec<Value>,
    permission_responses: Vec<Value>,
    refused_requests: Vec<String>,
    probes: u64,
    last_sequence: i64,
}

fn play(case: &Value) -> Result<Observed, Box<dyn std::error::Error>> {
    let mut session = StreamSession::default();
    let mut observed = Observed::default();
    let mut router = InboundRouter::default();
    for step in case.get("steps").and_then(Value::as_array).into_iter().flatten() {
        if text(step, "op")? == "write"
            && let Some(uuid) = step["frame"].get("uuid").and_then(Value::as_str)
        {
            router.remember_echo(uuid);
        }
    }
    let mut timed_out = false;
    for connection in list(case, "stream")? {
        observed.gets.push(session.request());
        let status = u16::try_from(number(connection, "status")?)?;
        match session.opened(status) {
            StreamOpen::Close(code) => {
                observed.closes.push(code);
                break;
            }
            StreamOpen::Retry => {
                observed.delays.push(milliseconds(session.lost(0.5)));
                continue;
            }
            StreamOpen::Read => {}
        }
        for chunk in list(connection, "chunks")? {
            for effect in session.push(&chunk_bytes(chunk)?) {
                match effect {
                    StreamEffect::Deliver(line) => match router.route(&line) {
                        Some(Inbound::User(message)) => observed.inbound.push(message),
                        Some(Inbound::ControlResponse(message)) => observed.permission_responses.push(message),
                        Some(Inbound::ControlRequest(request)) => {
                            observed.refused_requests.push(text(&request, "request_id")?);
                        }
                        // A cancel for work that is not running changes nothing the server can see, and a session
                        // that attaches has no notification buffer.
                        Some(Inbound::ControlCancel(_) | Inbound::QueuedNotification(_)) | None => {}
                    },
                    StreamEffect::Acknowledge { event_id } => {
                        observed.acks.push((event_id.clone(), "received".to_owned()));
                        observed.acks.push((event_id, "processed".to_owned()));
                    }
                    StreamEffect::HeartbeatProbe => observed.probes += 1,
                }
            }
        }
        if text(connection, "then")? == "end" {
            observed.delays.push(milliseconds(session.lost(0.5)));
        } else if !timed_out && let Some(clock) = case.get("clock") {
            timed_out = true;
            // The recorder bent the clocks: the liveness timer fired after 300 ms of real time, and a clock that
            // jumped reads 45 s plus its lateness, one that did not reads the 300 ms.
            let silence = |late: &Value| late.as_u64().map_or(300, |late| 45_000 + late);
            let wall = Duration::from_millis(silence(&clock["late_wall"]));
            let monotonic = Duration::from_millis(silence(&clock["late_mono"]));
            match session.liveness_expired(wall, monotonic, 0.5) {
                Reopen::AtOnce { .. } => observed.delays.push(0),
                Reopen::After(delay) => observed.delays.push(milliseconds(delay)),
            }
        } else {
            break;
        }
    }
    observed.last_sequence = session.last_sequence();
    Ok(observed)
}

fn recorded(case: &Value) -> Result<Observed, Box<dyn std::error::Error>> {
    let mut observed = Observed::default();
    for request in list(case, "requests")? {
        let method = text(request, "method")?;
        let path = text(request, "path")?;
        if method == "GET" && path.starts_with(STREAM_PATH) {
            let query = path
                .strip_prefix(STREAM_PATH)
                .and_then(|rest| rest.strip_prefix('?'))
                .map(str::to_owned);
            let last_event_id = list(request, "headers")?.iter().find_map(|header| {
                let pair = header.as_array()?;
                (pair.first()?.as_str()?.eq_ignore_ascii_case("last-event-id"))
                    .then(|| pair.get(1)?.as_str().map(str::to_owned))?
            });
            observed.gets.push(StreamRequest { query, last_event_id });
        } else if method == "POST" && path == "/worker/events/delivery" {
            let body: Value = serde_json::from_str(&text(request, "body")?)?;
            for update in list(&body, "updates")? {
                observed.acks.push((text(update, "event_id")?, text(update, "status")?));
            }
        } else if method == "POST" && path == "/worker/events" {
            let body: Value = serde_json::from_str(&text(request, "body")?)?;
            for event in list(&body, "events")? {
                let payload = &event["payload"];
                if text(payload, "type")? == "control_response" {
                    observed
                        .refused_requests
                        .push(text(&payload["response"], "request_id")?);
                }
            }
        }
    }
    observed.delays = case
        .get("reconnect_base_ms")
        .and_then(Value::as_array)
        .map(|delays| delays.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default();
    observed.closes = list(case, "closes")?
        .iter()
        .filter_map(Value::as_u64)
        .map(u16::try_from)
        .collect::<Result<_, _>>()?;
    observed.inbound = list(case, "inbound")?.clone();
    observed.permission_responses = list(case, "permission_responses")?.clone();
    observed.probes = number(case, "beats")?;
    observed.last_sequence = i64::try_from(number(case, "last_sequence")?)?;
    Ok(observed)
}

#[test]
fn every_recorded_stream_case_is_reproduced() -> TestResult {
    let goldens = goldens()?;
    let cases = list(&goldens, "stream")?;
    assert!(cases.len() >= 51, "only {} stream cases", cases.len());
    for case in cases {
        let name = text(case, "name")?;
        assert_eq!(recorded(case)?, play(case)?, "{name}");
    }
    Ok(())
}
