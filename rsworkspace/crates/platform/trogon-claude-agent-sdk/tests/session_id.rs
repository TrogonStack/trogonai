use trogon_claude_agent_sdk::SessionId;

#[test]
fn valid_opaque_values_are_preserved_without_format_restrictions() {
    for input in ["opaque-value", "unicode café value", "  "] {
        let value = SessionId::from(input);
        assert_eq!(value.as_str(), input);
        assert_eq!(value.clone(), value);
        assert_eq!(value.to_string(), input);
    }
}

#[test]
fn session_strings_can_be_owned_at_construction() {
    let input = String::from("café value");
    let value: SessionId = SessionId::from(input);
    assert_eq!(value.as_str(), "café value");
}

fn assert_upstream_session_round_trip(input: &str) {
    let value = SessionId::from(input);
    assert_eq!(value.as_str(), input);
    assert_eq!(value.to_string(), input);
    assert_eq!(value.clone(), value);
    let encoded = serde_json::json!({ "session_id": value.as_str() }).to_string();
    let decoded = serde_json::from_str::<serde_json::Value>(&encoded);
    assert!(matches!(decoded, Ok(message) if message["session_id"] == input));
    assert!(!encoded.contains('\0'));
    assert!(!encoded.contains('\n'));
}

#[test]
fn empty_session_strings_remain_lossless_in_json() {
    assert_upstream_session_round_trip("")
}

#[test]
fn nul_session_strings_remain_lossless_in_json() {
    assert_upstream_session_round_trip("before\0after")
}

#[test]
fn newline_session_strings_remain_lossless_in_json() {
    assert_upstream_session_round_trip("line\nidentifier")
}

#[test]
fn tab_session_strings_remain_lossless_in_json() {
    assert_upstream_session_round_trip("\t  ")
}

#[test]
fn del_session_strings_remain_lossless_in_json() {
    assert_upstream_session_round_trip("value\u{7f}")
}

#[test]
fn c1_control_session_strings_remain_lossless_in_json() {
    assert_upstream_session_round_trip("value\u{85}")
}
