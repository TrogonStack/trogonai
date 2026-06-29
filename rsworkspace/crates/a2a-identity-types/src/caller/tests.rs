use super::*;

#[test]
fn accepts_single_segment() {
    let caller = CallerId::new("alice").unwrap();
    assert_eq!(caller.as_str(), "alice");
}

#[test]
fn rejects_empty() {
    assert!(matches!(CallerId::new(""), Err(JwtError::InvalidCallerId)));
}

#[test]
fn rejects_dotted() {
    assert!(matches!(CallerId::new("a.b"), Err(JwtError::InvalidCallerId)));
}

#[test]
fn rejects_nats_wildcards() {
    assert!(matches!(CallerId::new("*"), Err(JwtError::InvalidCallerId)));
    assert!(matches!(CallerId::new(">"), Err(JwtError::InvalidCallerId)));
    assert!(matches!(CallerId::new("a*b"), Err(JwtError::InvalidCallerId)));
    assert!(matches!(CallerId::new("a>b"), Err(JwtError::InvalidCallerId)));
}

#[test]
fn rejects_whitespace() {
    assert!(matches!(CallerId::new("a b"), Err(JwtError::InvalidCallerId)));
    assert!(matches!(CallerId::new("a\tb"), Err(JwtError::InvalidCallerId)));
}

#[test]
fn serializes_transparent() {
    let caller = CallerId::new("alice").unwrap();
    let json = serde_json::to_string(&caller).unwrap();
    assert_eq!(json, "\"alice\"");
}

#[test]
fn deserializes_transparent_when_valid() {
    let caller: CallerId = serde_json::from_str("\"alice\"").unwrap();
    assert_eq!(caller.as_str(), "alice");
}

#[test]
fn deserialize_rejects_dotted_input() {
    assert!(serde_json::from_str::<CallerId>("\"a.b\"").is_err());
}

#[test]
fn deserialize_rejects_empty_input() {
    assert!(serde_json::from_str::<CallerId>("\"\"").is_err());
}
