use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const HELPERS: &str = include_str!("../tests/fixtures/sse_frames/helpers.json");
const FILTERS: &str = include_str!("../tests/fixtures/sse_frames/message_filters.json");

#[test]
fn text_markers_match_the_browser_predicate() -> TestResult {
    let helpers: Value = serde_json::from_str(HELPERS)?;
    let entries = helpers
        .get("markers")
        .and_then(Value::as_array)
        .ok_or("missing markers")?;
    assert!(entries.len() > 50);
    for entry in entries {
        let text = entry.get("text").and_then(Value::as_str).ok_or("missing text")?;
        let expected = entry.get("hi").and_then(Value::as_bool).ok_or("missing hi")?;
        assert_eq!(is_worker_output_text(text), expected, "{entry}");
    }
    Ok(())
}

#[test]
fn user_payload_checks_match_the_browser_predicates() -> TestResult {
    let entries: Vec<Value> = serde_json::from_str(FILTERS)?;
    assert!(entries.len() > 50);
    for entry in &entries {
        let source = entry.get("json").and_then(Value::as_str).ok_or("missing json")?;
        let payload: Value = serde_json::from_str(source)?;
        let payload = payload.as_object().ok_or("payload is not an object")?;
        let worker = entry.get("vi").and_then(Value::as_bool).ok_or("missing vi")?;
        let malformed = entry.get("gi").and_then(Value::as_bool).ok_or("missing gi")?;
        assert_eq!(is_worker_output_user(payload), worker, "{source}");
        assert_eq!(has_malformed_user_content(payload), malformed, "{source}");
    }
    Ok(())
}
