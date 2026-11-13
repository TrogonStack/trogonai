#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(serde_json_macro, reason = "Tests exercise exact upstream JSON fixtures.")
)]

//! The texts below were recorded from the real Python `_errors.py` (0.2.181) with
//! `.config/mise/lib/claude-sdk-freshness/error_text_goldens.py`.

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn object(value: serde_json::Value) -> Result<JsonObject, io::Error> {
    match value {
        serde_json::Value::Object(object) => Ok(object),
        other => Err(io::Error::other(format!("not an object: {other}"))),
    }
}

fn parse_error(line: &str) -> Result<serde_json::Error, io::Error> {
    serde_json::from_str::<serde_json::Value>(line)
        .err()
        .ok_or_else(|| io::Error::other("the line parsed"))
}

#[test]
fn cli_not_found_text_matches_the_python_constructor() {
    assert_eq!(python_cli_not_found_text(None, None), "Claude Code not found");
    assert_eq!(
        python_cli_not_found_text(None, Some("/opt/claude")),
        "Claude Code not found: /opt/claude"
    );
    assert_eq!(python_cli_not_found_text(Some("custom"), None), "custom");
    assert_eq!(
        python_cli_not_found_text(Some("custom"), Some("/opt/claude")),
        "custom: /opt/claude"
    );
    assert_eq!(python_cli_not_found_text(None, Some("")), "Claude Code not found");
}

#[test]
fn process_error_text_matches_the_python_constructor() {
    assert_eq!(python_process_error_text("failed", None, None), "failed");
    assert_eq!(
        python_process_error_text("failed", Some(7), None),
        "failed (exit code: 7)"
    );
    assert_eq!(
        python_process_error_text("failed", Some(0), None),
        "failed (exit code: 0)"
    );
    assert_eq!(
        python_process_error_text("failed", None, Some("boom")),
        "failed\nError output: boom"
    );
    assert_eq!(python_process_error_text("failed", None, Some("")), "failed");
    assert_eq!(
        python_process_error_text("failed", Some(7), Some("boom")),
        "failed (exit code: 7)\nError output: boom"
    );
}

#[test]
fn result_error_text_and_fields_match_the_python_constructor() -> TestResult {
    assert_eq!(ResultError::from_python("failed", None, None).python_text(), "failed");
    let error = ResultError::from_python(
        "failed",
        Some(object(serde_json::json!({"subtype": "error_max_turns"}))?),
        Some(1),
    );
    assert_eq!(error.python_text(), "failed (exit code: 1)");
    assert_eq!(error.to_string(), "failed");
    assert_eq!(error.exit_code(), Some(1));
    let error = ResultError::from_python(
        "failed",
        Some(object(serde_json::json!({
            "subtype": "error_max_turns",
            "errors": ["  a  ", "", 3, "b"],
            "result": "text",
            "api_error_status": 529,
            "terminal_reason": "max_turns",
            "session_id": "s"
        }))?),
        None,
    );
    assert_eq!(error.subtype(), Some("error_max_turns"));
    assert_eq!(error.errors(), ["a", "b"]);
    assert_eq!(error.result(), Some("text"));
    assert_eq!(error.api_error_status(), Some(529));
    assert_eq!(error.terminal_reason(), Some("max_turns"));
    assert_eq!(error.session_id(), Some("s"));
    Ok(())
}

#[test]
fn json_decode_text_matches_the_python_constructor() -> TestResult {
    let short = JsonDecodeError::from_line("{oops", parse_error("{oops")?);
    assert_eq!(short.to_string(), "Failed to decode JSON: {oops...");
    assert_eq!(short.line(), "{oops");
    let long = "x".repeat(120);
    assert_eq!(
        JsonDecodeError::from_line(&long, parse_error("{oops")?).to_string(),
        format!("Failed to decode JSON: {}...", "x".repeat(100))
    );
    let exact = "y".repeat(100);
    assert_eq!(
        JsonDecodeError::from_line(&exact, parse_error("{oops")?).to_string(),
        format!("Failed to decode JSON: {exact}...")
    );
    Ok(())
}
