use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const HELPERS: &str = include_str!("../tests/fixtures/sse_frames/helpers.json");

fn bytes(hex: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    (0..hex.len())
        .step_by(2)
        .map(|at| Ok(u8::from_str_radix(&hex[at..at + 2], 16)?))
        .collect()
}

#[test]
fn reasons_match_the_browser_reader() -> TestResult {
    let helpers: Value = serde_json::from_str(HELPERS)?;
    let entries = helpers
        .get("reasons")
        .and_then(Value::as_array)
        .ok_or("missing reasons")?;
    assert!(entries.len() > 30);
    for entry in entries {
        let body = bytes(entry.get("hex").and_then(Value::as_str).ok_or("missing hex")?)?;
        let expected = entry.get("reason").and_then(Value::as_str).map(str::to_owned);
        assert_eq!(http_error_reason(&body), expected, "{entry}");
    }
    Ok(())
}

#[test]
fn only_the_first_4096_bytes_are_read() {
    let mut body = br#"{"error":{"type":"x"},"pad":""#.to_vec();
    body.resize(5000, b'a');
    body.extend_from_slice(b"\"}");
    assert_eq!(http_error_reason(&body), None);
}

#[test]
fn bodies_without_a_reason_have_none() {
    assert_eq!(http_error_reason(b""), None);
    assert_eq!(http_error_reason(b"{}"), None);
    assert_eq!(http_error_reason(br#"{"error":"x"}"#), None);
    assert_eq!(
        http_error_reason(br#"{"error":{"resource":null,"type":"t"}}"#).as_deref(),
        Some("t")
    );
}

#[test]
fn the_error_reports_what_failed() {
    let error = SseHttpError::new("SSE connect failed", 404, Some("session".to_owned()));
    assert_eq!(error.to_string(), "SSE connect failed: HTTP 404");
    assert_eq!(error.name(), "SSEHttpError");
    assert_eq!(error.code(), 404);
    assert_eq!(error.reason(), Some("session"));
    assert!(error.is_permanent());
    assert!(!SseHttpError::new("x", 500, None).is_permanent());
    assert_eq!(SseHttpError::new("x", 500, None).reason(), None);
}
