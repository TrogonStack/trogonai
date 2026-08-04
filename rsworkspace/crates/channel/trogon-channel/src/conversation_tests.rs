use super::*;
use crate::agent_port::AgentSessionId;
use trogon_std::UuidV7Generator;

#[test]
fn generated_ids_are_v7_in_simple_form() {
    let id = ConversationId::generate(&UuidV7Generator);
    assert_eq!(id.as_str().len(), 32);
    assert!(id.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(id.as_str().chars().nth(12), Some('7'), "version nibble");
}

#[test]
fn generated_ids_sort_in_creation_order() {
    let first = ConversationId::generate(&UuidV7Generator);
    let second = ConversationId::generate(&UuidV7Generator);
    assert!(first.as_str() < second.as_str());
}

#[test]
fn conversation_id_from_string_round_trips_the_given_id() {
    let id = ConversationId::from_string("some-opaque-id").expect("valid");
    assert_eq!(id.as_str(), "some-opaque-id");
}

#[test]
fn conversation_id_display_renders_the_bare_id() {
    let id = ConversationId::from_string("some-opaque-id").expect("valid");
    assert_eq!(id.to_string(), "some-opaque-id");
}

#[test]
fn conversation_id_rejects_unsafe_tokens() {
    assert_eq!(
        ConversationId::from_string("a.b").unwrap_err(),
        EndpointError::InvalidCharacter('.')
    );
    assert_eq!(ConversationId::from_string("").unwrap_err(), EndpointError::Empty);
}

#[test]
fn conversation_id_deserialize_rejects_unsafe_tokens() {
    let err = serde_json::from_str::<ConversationId>("\"a.b\"").expect_err("dot is unsafe");
    assert!(err.to_string().contains("invalid character"), "{err}");
}

#[test]
fn agent_id_as_str_returns_the_constructed_id() {
    let agent = AgentId::new("sales-agent").expect("valid");
    assert_eq!(agent.as_str(), "sales-agent");
}

#[test]
fn agent_id_display_renders_the_bare_id() {
    let agent = AgentId::new("sales-agent").expect("valid");
    assert_eq!(agent.to_string(), "sales-agent");
}

#[test]
fn agent_id_rejects_unsafe_tokens() {
    assert_eq!(
        AgentId::new("sales.agent").unwrap_err(),
        EndpointError::InvalidCharacter('.')
    );
}

#[test]
fn agent_id_deserialize_rejects_unsafe_tokens() {
    let err = serde_json::from_str::<AgentId>("\"sales.agent\"").expect_err("dot is unsafe");
    assert!(err.to_string().contains("invalid character"), "{err}");
}

#[test]
fn agent_session_id_rejects_unsafe_tokens() {
    assert_eq!(
        AgentSessionId::new("sess.1").unwrap_err(),
        EndpointError::InvalidCharacter('.')
    );
}

#[test]
fn agent_session_id_deserialize_rejects_unsafe_tokens() {
    let err = serde_json::from_str::<AgentSessionId>("\"sess.1\"").expect_err("dot is unsafe");
    assert!(err.to_string().contains("invalid character"), "{err}");
}

#[test]
fn conversation_record_deserialize_rejects_a_corrupt_agent_id() {
    let err = serde_json::from_value::<ConversationRecord>(serde_json::json!({
        "principal": "user-1",
        "agent_id": "bad.id",
        "current_session": null,
        "created_at": 1,
        "last_activity_at": 1,
    }))
    .expect_err("corrupt agent_id");
    assert!(err.to_string().contains("invalid character"), "{err}");
}
