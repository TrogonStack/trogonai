use std::time::Duration;

use serde_json::{Value, json};
use trogon_sandbox_client::{ClientError, LogFollowEvent};
use trogon_sandbox_contract::{LogChunk, OutputStream};

use super::LogFollowBody;

fn body(item: Result<LogFollowEvent, ClientError>) -> Result<Value, serde_json::Error> {
    serde_json::to_value(LogFollowBody::from(&item))
}

#[test]
fn every_follow_event_has_its_wire_shape() -> Result<(), serde_json::Error> {
    let chunk = LogChunk {
        log_seq: 3,
        stream: OutputStream::Stdout,
        seq: 1,
        data: vec![104, 105],
    };
    let cases = [
        (
            Ok(LogFollowEvent::Chunk(chunk.clone())),
            json!({"type": "chunk", "log_seq": 3, "stream": "stdout", "seq": 1, "data": [104, 105]}),
        ),
        (
            Ok(LogFollowEvent::End {
                next_seq: 9,
                truncated: true,
            }),
            json!({"type": "end", "next_seq": 9, "truncated": true}),
        ),
        (Ok(LogFollowEvent::Unsupported), json!({"type": "unsupported"})),
        (
            Ok(LogFollowEvent::Reconnecting { attempt: 2 }),
            json!({"type": "reconnecting", "attempt": 2}),
        ),
        (
            Ok(LogFollowEvent::ResyncRequired { from_cursor: 7 }),
            json!({"type": "resync_required", "from_cursor": 7}),
        ),
    ];
    for (item, expected) in cases {
        assert_eq!(body(item)?, expected);
    }
    Ok(())
}

#[test]
fn a_client_failure_becomes_an_error_event() -> Result<(), serde_json::Error> {
    let event = body(Err(ClientError::Timeout(Duration::from_secs(2))))?;
    assert_eq!(event["type"], "error");
    assert_eq!(event["code"], "timeout");
    Ok(())
}
