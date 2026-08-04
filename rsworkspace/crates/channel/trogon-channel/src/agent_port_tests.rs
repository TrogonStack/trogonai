use super::*;

/// A session id is an endpoint token because it becomes part of one: it is
/// stored in `ConversationRecord` and printed back into the pipeline's logs.
#[test]
fn a_session_id_must_be_an_endpoint_token() {
    assert_eq!(AgentSessionId::new("sess-1").expect("valid").as_str(), "sess-1");
    assert_eq!(
        AgentSessionId::new("sess 1").unwrap_err(),
        EndpointError::InvalidCharacter(' ')
    );
    assert_eq!(AgentSessionId::new("").unwrap_err(), EndpointError::Empty);
}

/// Every log line the pipeline writes about a session (`session = %session`)
/// goes through this, so it has to print what the KV store holds.
#[test]
fn a_session_id_displays_as_the_token_it_wraps() {
    let session = AgentSessionId::new("sess-1").expect("valid");
    assert_eq!(session.to_string(), session.as_str());
}

/// The record is what a restarted bridge reads back, so an id that could not
/// have been constructed must not arrive through JSON either.
#[test]
fn deserializing_a_session_id_rejects_one_the_constructor_would_reject() {
    let ok: AgentSessionId = serde_json::from_str(r#""sess-1""#).expect("valid session id");
    assert_eq!(ok.as_str(), "sess-1");

    let err = serde_json::from_str::<AgentSessionId>(r#""sess 1""#).expect_err("unsafe id must not deserialize");
    assert!(err.to_string().contains("invalid character"), "{err}");
}
