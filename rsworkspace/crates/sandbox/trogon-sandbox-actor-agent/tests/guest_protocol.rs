use std::error::Error;

use trogon_sandbox_actor_agent::{
    ActorId, ActorIdError, GUEST_PROTOCOL_VERSION, GuestCallFailure, GuestEntropySeed, GuestProtocolError,
    PROTOCOL_REFUSED_STATUS, UNIMPLEMENTED_STATUS, admit_agent_version, admit_protocol_version,
    min_protocol_version_from_env, parse_min_protocol_version, parse_protocol_version,
};
use trogon_std::env::InMemoryEnv;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn an_actor_id_is_trimmed_and_never_empty() -> TestResult {
    let id = ActorId::new("  actor-1\n")?;
    assert_eq!(id.as_str(), "actor-1");
    assert_eq!(id.to_string(), "actor-1");
    assert_eq!(String::from(id), "actor-1");
    assert_eq!(ActorId::new(" \n"), Err(ActorIdError::Empty));
    assert_eq!(ActorIdError::Empty.to_string(), "actor id is empty");
    Ok(())
}

#[test]
fn an_actor_id_parses_from_the_wire() -> TestResult {
    let id: ActorId = serde_json::from_str("\"actor-2\"")?;
    assert_eq!(ActorId::try_from("actor-2".to_string())?, id);
    assert_eq!(serde_json::to_string(&id)?, "\"actor-2\"");
    assert!(serde_json::from_str::<ActorId>("\"  \"").is_err());
    Ok(())
}

#[test]
fn an_entropy_seed_never_prints_its_bytes() -> TestResult {
    let seed = GuestEntropySeed::new([7; GuestEntropySeed::LEN]);
    assert_eq!(seed.as_bytes(), &[7; 32]);
    assert_eq!(format!("{seed:?}"), "GuestEntropySeed([redacted])");
    Ok(())
}

#[test]
fn a_typed_unimplemented_body_allows_a_fallback_on_any_status() -> TestResult {
    let body = serde_json::to_vec(&GuestProtocolError::Unimplemented)?;
    for status in [200, 404, 426, 501] {
        let failure = GuestCallFailure::from_http(status, &body);
        assert_eq!(failure, GuestCallFailure::Unimplemented);
        assert!(failure.may_fall_back());
    }
    Ok(())
}

#[test]
fn a_typed_refusal_on_the_refusal_status_is_final() -> TestResult {
    let error = GuestProtocolError::VersionMalformed;
    let body = serde_json::to_vec(&error)?;
    let failure = GuestCallFailure::from_http(PROTOCOL_REFUSED_STATUS, &body);
    assert_eq!(failure, GuestCallFailure::Refused(error));
    assert!(!failure.may_fall_back());
    Ok(())
}

#[test]
fn the_unimplemented_status_alone_allows_a_fallback() {
    assert_eq!(
        GuestCallFailure::from_http(UNIMPLEMENTED_STATUS, b"not json"),
        GuestCallFailure::Unimplemented
    );
}

#[test]
fn any_other_reply_never_allows_a_fallback() -> TestResult {
    let refusal_elsewhere = serde_json::to_vec(&GuestProtocolError::VersionMalformed)?;
    for (status, body) in [(404, b"" as &[u8]), (500, b"{}"), (200, refusal_elsewhere.as_slice())] {
        let failure = GuestCallFailure::from_http(status, body);
        assert_eq!(failure, GuestCallFailure::UnexpectedReply { status });
        assert!(!failure.may_fall_back());
    }
    Ok(())
}

#[test]
fn a_minimum_override_falls_back_to_the_default_unless_it_parses() {
    assert_eq!(parse_min_protocol_version(None, 1), 1);
    assert_eq!(parse_min_protocol_version(Some(""), 1), 1);
    assert_eq!(parse_min_protocol_version(Some("x"), 1), 1);
    assert_eq!(parse_min_protocol_version(Some(" 0 "), 1), 0);
    let env = InMemoryEnv::new();
    assert_eq!(min_protocol_version_from_env(&env, "TEST_MINIMUM", 3), 3);
    env.set("TEST_MINIMUM", " 0 ");
    assert_eq!(min_protocol_version_from_env(&env, "TEST_MINIMUM", 3), 0);
}

#[test]
fn protocol_versions_are_read_from_headers() {
    assert_eq!(parse_protocol_version(None), Ok(0));
    assert_eq!(parse_protocol_version(Some(" 4 ")), Ok(4));
    assert_eq!(
        parse_protocol_version(Some("four")),
        Err(GuestProtocolError::VersionMalformed)
    );
}

#[test]
fn a_peer_below_the_minimum_is_refused_with_both_versions() {
    assert_eq!(admit_protocol_version(1, 1), Ok(()));
    assert_eq!(
        admit_protocol_version(0, 1),
        Err(GuestProtocolError::VersionTooOld {
            peer_version: 0,
            min_supported: 1,
            this_version: GUEST_PROTOCOL_VERSION,
        })
    );
}

#[test]
fn an_agent_answer_is_admitted_by_its_version_header() {
    assert_eq!(admit_agent_version(Some("2"), 1), Ok(2));
    assert_eq!(admit_agent_version(None, 0), Ok(0));
    assert!(matches!(
        admit_agent_version(None, 1),
        Err(GuestProtocolError::VersionTooOld { .. })
    ));
    assert_eq!(
        admit_agent_version(Some("?"), 1),
        Err(GuestProtocolError::VersionMalformed)
    );
}
