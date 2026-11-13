//! Replays `tests/fixtures/sdk_mcp/python_request_ids.json` through the scan, the marker and the Python dialect:
//! the control response line that `claude_agent_sdk` 0.2.181 over `mcp` 2.3.0 wrote for each id token.
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(serde_json_macro, reason = "Tests build the exact JSON-RPC shapes the CLI sends.")
)]

use super::*;
use crate::{
    CancellationToken, ControlRequestId, LocalMcpServer, LocalMcpServerOptions, McpServerDialect, McpServerName,
    SdkMcpServer, mcp_routing,
    wire::{self, runtime_payload::McpResponsePayload},
};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const FIXTURE: &str = include_str!("../tests/fixtures/sdk_mcp/python_request_ids.json");

fn cases() -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let fixture: Value = serde_json::from_str(FIXTURE)?;
    Ok(fixture
        .get("cases")
        .and_then(Value::as_array)
        .ok_or("missing cases")?
        .clone())
}

fn text<'a>(case: &'a Value, field: &str) -> Result<&'a str, Box<dyn std::error::Error>> {
    Ok(case.get(field).and_then(Value::as_str).ok_or("missing field")?)
}

fn line(server: &str, message: &str) -> String {
    format!(
        r#"{{"type":"control_request","request_id":"r","request":{{"subtype":"mcp_message","server_name":"{server}","message":{message}}}}}"#
    )
}

/// What the driver does with a line: scan it, parse the safe bytes, and take the message out.
fn read(frame: &str) -> Result<(McpWireIds, JsonObject), Box<dyn std::error::Error>> {
    let scan = McpWireScan::new(frame.as_bytes());
    let object: JsonObject = serde_json::from_slice(scan.frame(frame.as_bytes()))?;
    let message = object
        .get("request")
        .and_then(|request| request.get("message"))
        .and_then(Value::as_object)
        .ok_or("missing message")?
        .clone();
    Ok((scan.into_ids().unwrap_or_default(), message))
}

fn written(response: Value) -> Result<String, Box<dyn std::error::Error>> {
    let frame = wire::success_response(
        &ControlRequestId::from("r"),
        McpResponsePayload { mcp_response: response },
    )?;
    Ok(splice_markers(crate::python_json::to_string(&frame)?))
}

#[test]
fn a_server_that_is_not_found_echoes_every_recorded_id_as_python_wrote_it() -> TestResult {
    for case in cases()? {
        let token = text(&case, "token")?;
        let (ids, mut message) = read(&line(
            "nope",
            &format!(r#"{{"jsonrpc":"2.0","id":{token},"method":"ping"}}"#),
        ))?;
        ids.apply_to(&mut message);
        let mut response = mcp_routing::python_error(&message, -32601, "Server 'nope' not found".to_owned());
        ids.echo(&mut response);
        assert_eq!(written(response)?, text(&case, "not_found_line")?, "token {token:.40}");
    }
    Ok(())
}

#[tokio::test]
async fn the_python_dialect_answers_a_request_with_the_id_python_echoed() -> TestResult {
    for case in cases()? {
        let token = text(&case, "token")?;
        let dump = text(&case, "dump")?;
        let session = LocalMcpServer::from(LocalMcpServerOptions {
            dialect: McpServerDialect::Python,
            ..LocalMcpServerOptions::new(McpServerName::from("s"))
        })
        .open_session();
        for message in [
            json!({"jsonrpc":"2.0","id":9999,"method":"initialize","params":{
                "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"c","version":"1"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        ] {
            let message = serde_json::from_value(message)?;
            session.request(message, CancellationToken::new()).await?;
        }
        let (ids, mut message) = read(&line(
            "s",
            &format!(r#"{{"jsonrpc":"2.0","id":{token},"method":"ping"}}"#),
        ))?;
        ids.apply_to(&mut message);
        let answer = session.request(message, CancellationToken::new()).await?;
        let has_id = case
            .get("ping_has_id")
            .and_then(Value::as_bool)
            .ok_or("missing ping_has_id")?;
        assert_eq!(answer.is_some(), has_id, "token {token:.40}");
        if let Some(answer) = answer {
            let mut response = Value::Object(answer);
            ids.echo(&mut response);
            let expected = format!(r#""mcp_response": {{"jsonrpc": "2.0", "id": {dump}, "result": {{}}}}"#);
            assert!(written(response)?.contains(&expected), "token {token:.40}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_cancellation_names_a_big_id_by_the_integer_it_holds() -> TestResult {
    let session = LocalMcpServer::from(LocalMcpServerOptions {
        dialect: McpServerDialect::Python,
        ..LocalMcpServerOptions::new(McpServerName::from("s"))
    })
    .open_session();
    let frame = line(
        "s",
        r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":123456789012345678901234567890,"requestId":-0}}"#,
    );
    let (ids, mut message) = read(&frame)?;
    ids.apply_to(&mut message);
    assert_eq!(
        message.get("params").and_then(|params| params.get("requestId")),
        Some(&json!(0))
    );
    assert!(session.request(message, CancellationToken::new()).await?.is_none());
    Ok(())
}

#[test]
fn only_the_two_ids_of_an_mcp_message_are_read_and_the_last_duplicate_wins() -> TestResult {
    let scan = McpWireScan::new(
        line(
            "s",
            r#"{"id":5,"id":12345678901234567890123,"params":{"requestId":1e400,"other":{"id":99999999999999999999999}}}"#,
        )
        .as_bytes(),
    );
    let ids = scan.into_ids().ok_or("expected ids")?;
    assert_eq!(
        ids.id.as_ref().and_then(RawJsonId::integer),
        Some("12345678901234567890123")
    );
    assert_eq!(ids.requested.as_ref().map(RawJsonId::python_dump), Some("Infinity"));
    Ok(())
}

#[test]
fn an_earlier_duplicate_id_that_serde_json_refuses_does_not_fail_the_line() -> TestResult {
    let (ids, mut message) = read(&line(
        "s",
        r#"{"jsonrpc":"2.0","id":NaN,"id":7,"method":"notifications/cancelled","params":{"requestId":1e400,"requestId":-Infinity,"requestId":8}}"#,
    ))?;
    ids.apply_to(&mut message);
    assert_eq!(message.get("id"), Some(&json!(7)));
    assert_eq!(
        message.get("params").and_then(|params| params.get("requestId")),
        Some(&json!(8))
    );
    Ok(())
}

#[test]
fn lines_that_are_not_an_mcp_message_are_left_alone() {
    for frame in [
        r#"{"type":"control_request","request_id":"r","request":{"subtype":"can_use_tool","message":{"id":123456789012345678901234567890}}}"#.to_owned(),
        r#"{"type":"assistant","message":{"id":123456789012345678901234567890},"note":"mcp_message"}"#.to_owned(),
        line("s", r#"{"id":"text"}"#),
        line("s", r#"{"id":}"#),
        "not json mcp_message".to_owned(),
    ] {
        let scan = McpWireScan::new(frame.as_bytes());
        assert_eq!(scan.frame(frame.as_bytes()), frame.as_bytes(), "{frame}");
        assert_eq!(scan.into_ids(), None, "{frame}");
    }
}

#[test]
fn a_marker_is_spelled_out_where_the_line_is_written_and_nowhere_else() -> TestResult {
    let raw = RawJsonId::parse("123456789012345678901234567890")?;
    let value = dialect_value(&raw).ok_or("no value")?;
    assert_eq!(marker_text(&value).as_deref(), Some("123456789012345678901234567890"));
    assert_eq!(marker_text(&json!("123456789012345678901234567890")), None);
    assert_eq!(marker_text(&json!(1)), None);
    let text = crate::python_json::to_string(&json!({"id": value, "note": "plain \"text\""}))?;
    assert_eq!(
        splice_markers(text),
        r#"{"id": 123456789012345678901234567890, "note": "plain \"text\""}"#
    );
    assert_eq!(splice_markers("no marker here".to_owned()), "no marker here");
    Ok(())
}
