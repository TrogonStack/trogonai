use super::*;

/// A session id has to survive being spent as a subject token, which is how a
/// port addresses the session it names.
#[test]
fn a_session_id_must_be_a_subject_token() {
    assert_eq!(AgentSessionId::new("sess-1").expect("valid").as_str(), "sess-1");
    assert_eq!(
        AgentSessionId::new("sess 1").unwrap_err(),
        AgentSessionIdError::InvalidCharacter(' ')
    );
    assert_eq!(AgentSessionId::new("").unwrap_err(), AgentSessionIdError::Empty);
}

/// How long a handle may be is the transport's rule, not the channel's, so the
/// limit is read from there rather than restated here. An id that breaks it must
/// be refused for its length: an agent that has already minted the session is
/// told which rule it broke, and a handle sitting exactly at the limit can still
/// address one.
#[test]
fn a_session_id_too_long_for_a_subject_token_is_refused_for_its_length() {
    let limit = trogon_nats::constants::MAX_NATS_TOKEN_LENGTH;
    assert_eq!(
        AgentSessionId::new("s".repeat(limit)).expect("valid").as_str().len(),
        limit
    );
    assert_eq!(
        AgentSessionId::new("s".repeat(limit + 1)).unwrap_err(),
        AgentSessionIdError::TooLong(limit + 1)
    );
}

/// The bridge never mints these, so the alphabet its own keys are drawn from
/// has no say: an agent that names sessions the way ACP allows must not have
/// them refused after it has already opened one.
#[test]
fn a_session_id_accepts_what_a_channel_key_would_not() {
    for id in ["sess:1", "urn:acp:session:9", "sess/1", "sess+1"] {
        assert_eq!(AgentSessionId::new(id).expect("valid").as_str(), id);
    }
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
    assert_eq!(err.classify(), serde_json::error::Category::Data, "{err}");
}
