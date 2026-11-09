use trogon_claude_agent_sdk::{SessionId, SessionIdError};

#[test]
fn valid_opaque_values_are_preserved_without_format_restrictions() -> Result<(), SessionIdError> {
    for input in ["opaque-value", "unicode café value", "  "] {
        let value = SessionId::new(input)?;
        assert_eq!(value.as_str(), input);
        assert_eq!(value.clone(), value);
        assert_eq!(value.to_string(), input);
    }
    Ok(())
}

#[test]
fn empty_and_control_values_are_typed_construction_errors() {
    for input in ["", "value\nidentifier", "value\0identifier", "value\u{7f}"] {
        assert!(matches!(SessionId::new(input), Err(SessionIdError)));
    }
}

fn assert_upstream_session_round_trip(input: &str) -> Result<(), SessionIdError> {
    let value = SessionId::new(input)?;
    assert_eq!(value.as_str(), input);
    assert_eq!(value.to_string(), input);
    assert_eq!(value.clone(), value);
    let encoded = serde_json::json!({ "session_id": value.as_str() }).to_string();
    let decoded = serde_json::from_str::<serde_json::Value>(&encoded);
    assert!(matches!(decoded, Ok(message) if message["session_id"] == input));
    assert!(!encoded.contains('\0'));
    assert!(!encoded.contains('\n'));
    Ok(())
}

#[test]
fn empty_session_strings_remain_lossless_in_json() -> Result<(), SessionIdError> {
    assert_upstream_session_round_trip("")
}

#[test]
fn nul_session_strings_remain_lossless_in_json() -> Result<(), SessionIdError> {
    assert_upstream_session_round_trip("before\0after")
}

#[test]
fn newline_session_strings_remain_lossless_in_json() -> Result<(), SessionIdError> {
    assert_upstream_session_round_trip("line\nidentifier")
}

#[test]
fn tab_session_strings_remain_lossless_in_json() -> Result<(), SessionIdError> {
    assert_upstream_session_round_trip("\t  ")
}

#[test]
fn del_session_strings_remain_lossless_in_json() -> Result<(), SessionIdError> {
    assert_upstream_session_round_trip("value\u{7f}")
}

#[test]
fn c1_control_session_strings_remain_lossless_in_json() -> Result<(), SessionIdError> {
    assert_upstream_session_round_trip("value\u{85}")
}
