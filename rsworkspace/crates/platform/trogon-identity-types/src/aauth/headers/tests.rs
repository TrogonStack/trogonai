use super::*;

#[test]
fn capabilities_parse_and_render_round_trip() {
    let raw = "interaction, clarification, payment";
    let caps = Capabilities::parse(raw);
    assert_eq!(
        caps.0,
        vec![Capability::Interaction, Capability::Clarification, Capability::Payment]
    );
    assert_eq!(caps.to_header_value(), raw);
}

#[test]
fn capabilities_parse_unknown_value_as_other() {
    let caps = Capabilities::parse("interaction, quantum-teleport");
    assert_eq!(
        caps.0,
        vec![Capability::Interaction, Capability::Other("quantum-teleport".into())]
    );
}

#[test]
fn capabilities_ignores_unknown_parameters() {
    let caps = Capabilities::parse("interaction;future-param=1, payment");
    assert_eq!(caps.0, vec![Capability::Interaction, Capability::Payment]);
}

#[test]
fn capability_other_as_str_returns_the_raw_token() {
    let cap = Capability::Other("quantum-teleport".into());
    assert_eq!(cap.as_str(), "quantum-teleport");
}

#[test]
fn capabilities_to_header_value_round_trips_unknown_capability() {
    let raw = "interaction, quantum-teleport";
    let caps = Capabilities::parse(raw);
    assert_eq!(caps.to_header_value(), raw);
}

#[test]
fn capabilities_contains_checks_membership() {
    let caps = Capabilities::parse("interaction");
    assert!(caps.contains(&Capability::Interaction));
    assert!(!caps.contains(&Capability::Payment));
}

#[test]
fn access_parses_opaque_token() {
    let access = Access::parse("wrapped-opaque-token-value").unwrap();
    assert_eq!(access.to_header_value(), "wrapped-opaque-token-value");
    assert_eq!(
        access.to_authorization_header_value(),
        "AAuth wrapped-opaque-token-value"
    );
}

#[test]
fn access_rejects_empty_value() {
    assert_eq!(Access::parse(""), Err(AccessParseError::Empty));
}

#[test]
fn access_rejects_whitespace_and_control_characters() {
    assert_eq!(Access::parse("has space"), Err(AccessParseError::InvalidCharacters));
    assert_eq!(Access::parse("has\ttab"), Err(AccessParseError::InvalidCharacters));
    assert_eq!(Access::parse("has\ncontrol"), Err(AccessParseError::InvalidCharacters));
}
