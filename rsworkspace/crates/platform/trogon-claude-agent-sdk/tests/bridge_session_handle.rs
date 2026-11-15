//! Replays the recorded `handle` cases (written by `.config/mise/lib/claude-sdk-freshness/bridge_worker_goldens.mjs`
//! against the handle the real `attachBridgeSession` returns) through the methods of [`BridgeSession`] the host calls,
//! and the state and delivery calls the older sections make through the same handle.
//!
//! The upstream handle uploads in the background, so a write that follows another one while its request is in flight
//! joins the next batch. The replay models that the same way: the first write of a step goes out as a request of its
//! own, the ones after it wait for the next one.

#[path = "support/bridge_session_server.rs"]
mod server;

use serde_json::{Map, Value};
use server::*;
use trogon_claude_agent_sdk::{DeliveryStatus, ReconnectOptions, SessionState, StreamOpen, StreamRequest};

const GOLDENS: &str = include_str!("fixtures/bridge_worker/goldens.json");

fn goldens() -> Result<Value, serde_json::Error> {
    serde_json::from_str(GOLDENS)
}

fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.char_indices().all(|(at, character)| match at {
            8 | 13 | 18 | 23 => character == '-',
            _ => character.is_ascii_hexdigit(),
        })
}

/// The recorder replaces the uuids the client made up with `<uuid>`; so does the replay.
fn hide_uuids(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map.iter_mut() {
                match inner {
                    Value::String(text) if key == "uuid" && is_uuid(text) => *text = "<uuid>".to_owned(),
                    other => hide_uuids(other),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(hide_uuids),
        _ => {}
    }
}

fn hidden(request: &Captured) -> Result<Captured, serde_json::Error> {
    let mut request = request.clone();
    if request.path == "/worker/events" {
        let mut body: Value = serde_json::from_str(&request.body)?;
        hide_uuids(&mut body);
        request.body = body.to_string();
    }
    Ok(request)
}

fn object(value: &Value) -> Result<Map<String, Value>, String> {
    value
        .as_object()
        .cloned()
        .ok_or_else(|| format!("not an object: {value}"))
}

fn state_of(name: &str) -> Result<SessionState, String> {
    match name {
        "idle" => Ok(SessionState::Idle),
        "running" => Ok(SessionState::Running),
        "requires_action" => Ok(SessionState::RequiresAction),
        other => Err(format!("unknown state {other}")),
    }
}

fn status_of(name: &str) -> Result<DeliveryStatus, String> {
    match name {
        "received" => Ok(DeliveryStatus::Received),
        "processing" => Ok(DeliveryStatus::Processing),
        "processed" => Ok(DeliveryStatus::Processed),
        other => Err(format!("unknown status {other}")),
    }
}

/// One write the host made: a plain frame, or the call named by `via`.
fn emit(session: &mut Session, item: &Value) -> TestResult {
    match field(item, "via").as_str() {
        None => session.write(object(item)?),
        Some("sendControlRequest") => session.send_control_request(object(&item["payload"])?),
        Some("sendControlResponse") => session.send_control_response(object(&item["payload"])?),
        Some("sendControlCancelRequest") => {
            session.send_control_cancel_request(&text(&item["payload"], "request_id")?);
        }
        Some(other) => return Err(format!("unknown call {other}").into()),
    }
    Ok(())
}

/// The writes of a step: the first alone, then the rest while that request is still unanswered.
async fn emit_all(session: &mut Session, items: &[Value]) -> TestResult {
    for (at, item) in items.iter().enumerate() {
        emit(session, item)?;
        if at == 0 {
            session.pump_attempts(Some(1)).await;
        }
    }
    Ok(())
}

/// Reads the next scripted connection of the case, as the upstream reader did, and records the request it made.
async fn read_connection(
    case: &Value,
    session: &mut Session,
    next: &mut usize,
    gets: &mut Vec<StreamRequest>,
) -> TestResult {
    let Some(connection) = list(case, "stream").ok().and_then(|stream| stream.get(*next)) else {
        return Ok(());
    };
    *next += 1;
    let Some(request) = session.stream_request() else {
        return Ok(());
    };
    gets.push(request);
    if !matches!(
        session.stream_opened(u16::try_from(number(connection, "status")?)?),
        StreamOpen::Read
    ) {
        return Ok(());
    }
    for chunk in list(connection, "chunks")? {
        session.stream_push(&chunk_bytes(chunk)?).await;
        session.settle_control().await;
        session.pump().await;
    }
    Ok(())
}

struct Replayed {
    requests: Vec<Captured>,
    gets: Vec<StreamRequest>,
    closes: Vec<u16>,
}

async fn replay(case: &Value) -> Result<Replayed, Box<dyn std::error::Error>> {
    let (base, log) = serve(case).await?;
    let (attached, hosted) = attach(case, &base).await?;
    let mut session = attached.map_err(|error| error.to_string())?;
    let (mut next, mut gets) = (0, Vec::new());
    read_connection(case, &mut session, &mut next, &mut gets).await?;
    for step in list(case, "steps")? {
        let items = field(step, "writes").as_array().cloned().unwrap_or_default();
        match text(step, "op")?.as_str() {
            "events" => {
                emit_all(&mut session, &items).await?;
                session.pump().await;
            }
            "result" => {
                session.send_result();
                session.pump().await;
            }
            "flush" => {
                for item in &items {
                    emit(&mut session, item)?;
                }
                session.flush().await;
            }
            "reconnect" => {
                emit_all(&mut session, &items).await?;
                let options = ReconnectOptions {
                    api_base_url: field(step, "badBase")
                        .as_str()
                        .map_or_else(|| base.clone(), str::to_owned),
                    ingress_token: "tok2".into(),
                    epoch: field(step, "epoch").as_i64(),
                };
                if field(step, "thenClose").as_bool() == Some(true) {
                    session.reconnect_transport_closed_meanwhile(options).await?;
                    continue;
                }
                match session.reconnect_transport(options).await {
                    Ok(()) => assert!(field(case, "rejected").is_null(), "the recorded rebuild was rejected"),
                    Err(error) => {
                        assert_eq!(
                            Some(error.to_string()),
                            field(case, "rejected").as_str().map(str::to_owned)
                        );
                        continue;
                    }
                }
                read_connection(case, &mut session, &mut next, &mut gets).await?;
                session.pump().await;
            }
            "close" => {
                session.close().await;
                if field(step, "again").as_bool() == Some(true) {
                    session.close().await;
                }
            }
            "state" => {
                session.report_state(state_of(&text(step, "state")?)?);
                session.pump().await;
            }
            "metadata" => {
                session.report_metadata(&object(&step["value"])?);
                session.pump().await;
            }
            "delivery" => {
                for report in list(step, "reports")? {
                    let (id, status) = (text_at(report, 0)?, text_at(report, 1)?);
                    session.report_delivery(&id, status_of(&status)?).await;
                }
                session.pump().await;
            }
            other => return Err(format!("unknown step {other}").into()),
        }
    }
    session.pump().await;
    Ok(Replayed {
        requests: requests_of(&log),
        gets,
        closes: taken(&hosted.closes),
    })
}

fn text_at(value: &Value, at: usize) -> Result<String, String> {
    value
        .get(at)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("missing item {at} in {value}"))
}

fn recorded_requests(case: &Value) -> Result<Vec<&Value>, String> {
    Ok(list(case, "requests")?
        .iter()
        .filter(|request| {
            !field(request, "path")
                .as_str()
                .is_some_and(|path| path.starts_with(STREAM_PATH))
        })
        .collect())
}

fn only<'a>(requests: &'a [&Value], puts: bool) -> Vec<&'a Value> {
    requests
        .iter()
        .copied()
        .filter(|request| (field(request, "method") == "PUT") == puts)
        .collect()
}

/// The service sees the requests of one worker in the order it made them, but a rebuilt transport registers while the
/// events it took over are already on their way, so the two kinds are compared apart. Upstream races there itself:
/// the new worker's PUT and the posting of the adopted events are started together, so no order is recorded.
async fn check(case: &Value) -> TestResult {
    let name = text(case, "name")?;
    assert!(field(case, "rejection").is_null(), "{name} was rejected");
    let replayed = replay(case).await?;
    let wanted = recorded_requests(case)?;
    let sent = replayed
        .requests
        .iter()
        .filter(|request| !request.path.starts_with(STREAM_PATH))
        .map(hidden)
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(wanted.len(), sent.len(), "{name} request count");
    for puts in [true, false] {
        let expected = only(&wanted, puts);
        let actual: Vec<&Captured> = sent
            .iter()
            .filter(|request| (request.method == "PUT") == puts)
            .collect();
        assert_eq!(expected.len(), actual.len(), "{name} request count (puts: {puts})");
        for (expected, actual) in expected.into_iter().zip(actual) {
            assert_request(&name, expected, actual);
        }
    }
    let streams: Vec<StreamRequest> = {
        let mut gets = Vec::new();
        for request in list(case, "requests")? {
            let path = text(request, "path")?;
            if text(request, "method")? == "GET" && path.starts_with(STREAM_PATH) {
                let last_event_id = list(request, "headers")?.iter().find_map(|header| {
                    let pair = header.as_array()?;
                    (pair.first()?.as_str()?.eq_ignore_ascii_case("last-event-id"))
                        .then(|| pair.get(1)?.as_str().map(str::to_owned))?
                });
                let query = path
                    .strip_prefix(STREAM_PATH)
                    .and_then(|rest| rest.strip_prefix('?'))
                    .map(str::to_owned);
                gets.push(StreamRequest { query, last_event_id });
            }
        }
        gets
    };
    assert_eq!(streams, replayed.gets, "{name} stream requests");
    assert_eq!(
        list(case, "closes")?
            .iter()
            .filter_map(Value::as_u64)
            .map(|code| u16::try_from(code).unwrap_or_default())
            .collect::<Vec<_>>(),
        replayed.closes,
        "{name} closes"
    );
    Ok(())
}

#[tokio::test]
async fn every_recorded_handle_case_is_reproduced() -> TestResult {
    let goldens = goldens()?;
    let cases = list(&goldens, "handle")?;
    assert!(cases.len() >= 18, "only {} handle cases", cases.len());
    for case in cases {
        check(case).await?;
    }
    Ok(())
}

/// The state, metadata and delivery calls whose recorded effect is one request each, replayed through the handle.
const SHARED_CASES: [(&str, &str); 9] = [
    ("state", "running"),
    ("state", "idle_is_not_repeated"),
    ("state", "metadata"),
    ("state", "metadata_blocked_summary"),
    ("events", "a_write_is_posted_with_the_epoch"),
    ("events", "a_missing_uuid_is_filled_in"),
    ("delivery", "a_received_ack_is_posted_with_the_epoch"),
    ("delivery", "a_processed_ack_is_posted"),
    ("delivery", "a_processing_ack_is_posted"),
];

#[tokio::test]
async fn the_state_event_and_delivery_calls_are_reproduced_through_the_handle() -> TestResult {
    let goldens = goldens()?;
    for (section, name) in SHARED_CASES {
        let case = list(&goldens, section)?
            .iter()
            .find(|case| case["name"] == name)
            .ok_or_else(|| format!("no {section} case {name}"))?;
        check(case).await?;
    }
    Ok(())
}
