//! The upload task of an attached session sends what the handle queued without anyone calling `pump`: the recorded
//! single-write cases of the handle, state, event and delivery sections are replayed through it, and the task ends
//! the session on a refusal for good and returns when the host closes it.

#[path = "support/bridge_session_server.rs"]
mod server;

use serde_json::{Map, Value, json};
use server::*;
use trogon_claude_agent_sdk::{DeliveryStatus, SessionState};

const GOLDENS: &str = include_str!("fixtures/bridge_worker/goldens.json");
const SECTIONS: [&str; 4] = ["state", "events", "delivery", "handle"];
const OPS: [&str; 5] = ["events", "result", "state", "metadata", "delivery"];

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

fn text_at(value: &Value, at: usize) -> Result<String, String> {
    value
        .get(at)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("missing item {at} in {value}"))
}

/// A case the upload task replays on its own: no read stream, one write per step.
fn replayable(case: &Value) -> bool {
    let steps = queue(case, "steps");
    case.get("stream").is_none()
        && field(case, "rejection").is_null()
        && steps
            .iter()
            .all(|step| OPS.contains(&field(step, "op").as_str().unwrap_or_default()))
        && steps
            .iter()
            .all(|step| queue(step, "writes").len() <= 1 && queue(step, "reports").len() <= 1)
}

async fn call(session: &mut Session, step: &Value) -> TestResult {
    match text(step, "op")?.as_str() {
        "events" => {
            for item in queue(step, "writes") {
                match field(&item, "via").as_str() {
                    None => session.write(object(&item)?),
                    Some("sendControlRequest") => session.send_control_request(object(&item["payload"])?),
                    Some("sendControlResponse") => session.send_control_response(object(&item["payload"])?),
                    Some("sendControlCancelRequest") => {
                        session.send_control_cancel_request(&text(&item["payload"], "request_id")?);
                    }
                    Some(other) => return Err(format!("unknown call {other}").into()),
                }
            }
        }
        "result" => session.send_result(),
        "state" => session.report_state(state_of(&text(step, "state")?)?),
        "metadata" => session.report_metadata(&object(&step["value"])?),
        "delivery" => {
            for report in queue(step, "reports") {
                let status = status_of(&text_at(&report, 1)?)?;
                session.report_delivery(&text_at(&report, 0)?, status).await;
            }
        }
        other => return Err(format!("unknown step {other}").into()),
    }
    Ok(())
}

fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.char_indices().all(|(at, character)| match at {
            8 | 13 | 18 | 23 => character == '-',
            _ => character.is_ascii_hexdigit(),
        })
}

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

async fn check(case: &Value) -> TestResult {
    let name = text(case, "name")?;
    let (base, log) = serve(case).await?;
    let (attached, _hosted) = attach(case, &base).await?;
    let mut session = attached.map_err(|error| error.to_string())?;
    session.spawn_uploads();
    for step in queue(case, "steps") {
        call(&mut session, &step).await?;
        session.uploaded().await;
    }
    let mut sent = requests_of(&log);
    for request in &mut sent {
        if request.path == "/worker/events" {
            let mut body: Value = serde_json::from_str(&request.body)?;
            hide_uuids(&mut body);
            request.body = body.to_string();
        }
    }
    let wanted = list(case, "requests")?;
    assert_eq!(wanted.len(), sent.len(), "{name} request count");
    for puts in [true, false] {
        let expected = wanted
            .iter()
            .filter(|request| (field(request, "method") == "PUT") == puts);
        let actual = sent.iter().filter(|request| (request.method == "PUT") == puts);
        assert_eq!(
            expected.clone().count(),
            actual.clone().count(),
            "{name} count (puts: {puts})"
        );
        for (expected, actual) in expected.zip(actual) {
            assert_request(&name, expected, actual);
        }
    }
    Ok(())
}

#[tokio::test]
async fn the_recorded_single_write_cases_upload_without_a_pump() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let mut replayed = 0;
    for section in SECTIONS {
        for case in list(&goldens, section)?.iter().filter(|case| replayable(case)) {
            check(case).await?;
            replayed += 1;
        }
    }
    assert!(replayed >= 40, "only {replayed} cases replayed");
    Ok(())
}

fn refusing_case() -> Value {
    json!({
        "name": "refused",
        "epoch_option": 6,
        "interval_ms": 600_000,
        "events": [{"status": 409, "headers": [["x-ccr-conflict-reason", "superseded_by_worker"]], "body": ""}],
    })
}

#[tokio::test]
async fn a_refusal_for_good_ends_the_session_once_from_the_task() -> TestResult {
    let case = refusing_case();
    let (base, log) = serve(&case).await?;
    let (attached, hosted) = attach(&case, &base).await?;
    let mut session = attached.map_err(|error| error.to_string())?;
    session.spawn_uploads();
    session.spawn_uploads();
    let mut frame = Map::new();
    frame.insert("type".to_owned(), json!("assistant"));
    frame.insert("uuid".to_owned(), json!("u1"));
    session.write(frame.clone());
    session.uploaded().await;
    assert_eq!(vec![4090], taken(&hosted.closes));
    assert!(!session.is_connected());
    let posts = |log: &Log| {
        requests_of(log)
            .iter()
            .filter(|request| request.path == "/worker/events")
            .count()
    };
    assert_eq!(1, posts(&log));
    frame.insert("uuid".to_owned(), json!("u2"));
    session.write(frame);
    session.uploaded().await;
    assert_eq!(1, posts(&log));
    assert_eq!(vec![4090], taken(&hosted.closes));
    Ok(())
}

#[tokio::test]
async fn the_task_returns_when_the_host_closes_and_nothing_is_sent_after() -> TestResult {
    let case = json!({"name": "closing", "epoch_option": 6, "interval_ms": 600_000});
    let (base, log) = serve(&case).await?;
    let (attached, _hosted) = attach(&case, &base).await?;
    let mut session = attached.map_err(|error| error.to_string())?;
    session.spawn_uploads();
    session.report_state(SessionState::Running);
    session.uploaded().await;
    session.close().await;
    session.join_uploads().await;
    let before = requests_of(&log).len();
    session.report_state(SessionState::Idle);
    session.write(Map::new());
    session.join_uploads().await;
    assert_eq!(before, requests_of(&log).len());
    Ok(())
}
