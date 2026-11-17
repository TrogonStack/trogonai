#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(serde_json_macro, reason = "The goldens compare the events as JSON values.")
)]

//! Replays whole sessions through the SSE parser and the frame filter and compares every event, drop count, cursor
//! and request-id set with what the browser transport did for the same bytes in Node
//! (`.config/mise/lib/claude-sdk-freshness/sse_frames_oracle.mjs` writes the goldens).

use serde_json::{Map, Value, json};
use trogon_claude_agent_sdk::{SseCcrEvent, SseEvent, SseFrameFilter, SseLogLevel, SseParser};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const SESSIONS: &str = include_str!("fixtures/sse_frames/sessions.json");
const PLACEHOLDER_UUID: &str = "00000000-0000-4000-8000-000000000001";

fn ccr_json(ccr: Option<&SseCcrEvent>) -> Value {
    let Some(ccr) = ccr else {
        return Value::Null;
    };
    let mut object = Map::new();
    object.insert("event_id".to_owned(), json!(ccr.event_id));
    object.insert("sequence_num".to_owned(), json!(ccr.sequence_num));
    if let Some(created_at) = &ccr.created_at {
        object.insert("created_at".to_owned(), json!(created_at));
    }
    if let Some(source) = &ccr.source {
        object.insert("source".to_owned(), json!(source));
    }
    Value::Object(object)
}

fn delivered(filter: &SseFrameFilter) -> Value {
    filter.last_sequence_num().map_or(Value::Null, |value| json!(value))
}

fn event_json(event: &SseEvent) -> Value {
    match event {
        SseEvent::Message { message, ccr_event } => {
            json!({"k": "message", "message": message, "ccr": ccr_json(ccr_event.as_ref())})
        }
        SseEvent::CatchUpTruncated => json!({"k": "catch_up"}),
        SseEvent::DeliveryUpdate(update) => {
            let mut object = Map::new();
            object.insert("event_id".to_owned(), json!(update.event_id));
            object.insert("status".to_owned(), json!(update.status));
            if let Some(timestamp) = &update.timestamp {
                object.insert("timestamp".to_owned(), json!(timestamp));
            }
            json!({"k": "delivery", "update": object})
        }
        SseEvent::Log(record) => {
            let level = match record.level {
                SseLogLevel::Debug => "debug",
                SseLogLevel::Warn => "warn",
            };
            json!({"k": "log", "level": level, "text": record.text})
        }
    }
}

fn id_set(ids: &[&str]) -> Value {
    let first: Vec<_> = ids.iter().take(2).collect();
    let last: Vec<_> = ids.iter().skip(ids.len().saturating_sub(2)).collect();
    json!({"length": ids.len(), "first": first, "last": last})
}

fn replay(session: &Value) -> Result<(SseFrameFilter, Vec<Value>), Box<dyn std::error::Error>> {
    let from = session.get("from").and_then(Value::as_f64);
    let mut filter = SseFrameFilter::new(from);
    let mut parser = SseParser::new();
    let mut events = Vec::new();
    let steps = session
        .get("steps")
        .and_then(Value::as_array)
        .ok_or("session has no steps")?;
    for step in steps {
        if let Some(chunk) = step.get("chunk").and_then(Value::as_str) {
            for frame in parser.push(chunk.as_bytes()) {
                for event in filter.handle_frame(&frame) {
                    events.push(event_json(&event));
                    if let SseEvent::Message { ccr_event, .. } = &event {
                        filter.mark_delivered(ccr_event.as_ref());
                        events.push(json!({"k": "delivered", "value": delivered(&filter)}));
                    }
                }
            }
        } else if let Some(line) = step.get("write").and_then(Value::as_str) {
            match filter.prepare_write(line, || PLACEHOLDER_UUID.to_owned()) {
                Ok(write) => {
                    if let Some(log) = &write.log {
                        events.push(event_json(&SseEvent::Log(log.clone())));
                    }
                    events.push(json!({"k": "post", "body": write.post_body("session-1")?}));
                }
                Err(_) => events.push(json!({"k": "write_error", "name": "SyntaxError"})),
            }
        } else if let Some(id) = step.get("expect").and_then(Value::as_str) {
            if let Some(log) = filter.expect_control_response(id) {
                events.push(event_json(&SseEvent::Log(log)));
            }
        } else {
            return Err(format!("unknown step {step}").into());
        }
    }
    Ok((filter, events))
}

fn without_transport_fields(event: &Value) -> Value {
    if event.get("k").and_then(Value::as_str) != Some("post") {
        return event.clone();
    }
    json!({"k": "post", "body": event.get("body")})
}

#[test]
fn every_session_ends_where_the_browser_transport_did() -> TestResult {
    let entries: Vec<Value> = serde_json::from_str(SESSIONS)?;
    assert!(entries.len() > 250, "the corpus shrank to {} sessions", entries.len());
    for entry in &entries {
        let session = entry.get("session").ok_or("entry has no session")?;
        let name = session.get("name").and_then(Value::as_str).unwrap_or("unnamed");
        let (filter, events) = replay(session)?;
        let expected: Vec<Value> = entry
            .get("events")
            .and_then(Value::as_array)
            .ok_or("entry has no events")?
            .iter()
            .map(without_transport_fields)
            .collect();
        assert_eq!(events, expected, "events of {name}");

        let counts = filter.drop_counts();
        let order: Vec<&str> = counts.iter().map(|(category, _)| category.as_str()).collect();
        let golden_order: Vec<&str> = entry
            .get("drop_order")
            .and_then(Value::as_array)
            .ok_or("entry has no drop_order")?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(order, golden_order, "drop order of {name}");
        let drops: Map<String, Value> = counts
            .iter()
            .map(|(category, count)| (category.as_str().to_owned(), json!(count)))
            .collect();
        assert_eq!(Some(&Value::Object(drops)), entry.get("drops"), "drops of {name}");

        assert_eq!(json!(filter.resume_sequence_num()), entry["cursor"], "cursor of {name}");
        assert_eq!(delivered(&filter), entry["delivered"], "delivered of {name}");
        let issued: Vec<&str> = filter.issued_request_ids().collect();
        assert_eq!(id_set(&issued), entry["issued"], "issued of {name}");
        let expected_ids: Vec<&str> = filter.expected_request_ids().collect();
        assert_eq!(id_set(&expected_ids), entry["expected"], "expected of {name}");
    }
    Ok(())
}
