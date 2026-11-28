use serde_json::json;
use trogon_sandbox_ws_gateway::WsFrame;

type TestResult = Result<(), serde_json::Error>;

#[test]
fn unclassified_error_fields_are_omitted_from_the_wire() -> TestResult {
    let wire = serde_json::to_value(WsFrame::response_err("1", "internal", "x"))?;
    assert_eq!(wire["error"], json!({"code": "internal", "message": "x"}));
    Ok(())
}
