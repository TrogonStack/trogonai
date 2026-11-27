use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_ssh_gateway::{
    SSH_ATTACH_FILTER, SSH_DETACH_FILTER, SshAttachRefusal, SshAttachResponse, SshDetachResponse, SshSessionId,
    SshSessionIdError, ssh_attach, ssh_detach, ssh_stream_from_worker, ssh_stream_to_worker,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn session_id_accepts_its_grammar() -> TestResult {
    let id: SshSessionId = "abc-DEF_123".parse()?;
    assert_eq!(id.as_str(), "abc-DEF_123");
    assert_eq!(id.to_string(), "abc-DEF_123");
    assert_eq!(String::from(id), "abc-DEF_123");
    assert_eq!(SshSessionId::try_from("x".to_string())?.as_str(), "x");
    Ok(())
}

#[test]
fn session_id_refuses_bad_lengths_and_characters() {
    assert_eq!(
        SshSessionId::new(""),
        Err(SshSessionIdError::InvalidLength {
            max: SshSessionId::MAX_LEN
        })
    );
    assert_eq!(
        SshSessionId::new("a".repeat(SshSessionId::MAX_LEN + 1)),
        Err(SshSessionIdError::InvalidLength {
            max: SshSessionId::MAX_LEN
        })
    );
    assert_eq!(SshSessionId::new("a.b"), Err(SshSessionIdError::InvalidCharacters));
    assert_eq!(SshSessionId::new("a b"), Err(SshSessionIdError::InvalidCharacters));
}

#[test]
fn session_id_json_enforces_the_grammar() -> TestResult {
    let id: SshSessionId = serde_json::from_str("\"abc\"")?;
    assert_eq!(serde_json::to_string(&id)?, "\"abc\"");
    assert!(serde_json::from_str::<SshSessionId>("\"a.b\"").is_err());
    Ok(())
}

#[test]
fn generated_session_ids_are_valid_and_distinct() -> TestResult {
    let first = SshSessionId::generate();
    let second = SshSessionId::generate();
    assert_ne!(first, second);
    SshSessionId::new(first.as_str())?;
    Ok(())
}

#[test]
fn subjects_name_the_lease_and_session() -> TestResult {
    let lease = LeaseId::new("lease-1");
    let session: SshSessionId = "s1".parse()?;
    assert_eq!(ssh_attach(&lease), "sandbox.ssh.attach.lease-1");
    assert_eq!(ssh_detach(&lease), "sandbox.ssh.detach.lease-1");
    assert_eq!(ssh_stream_to_worker(&session), "sandbox.ssh.stream.s1.to_worker");
    assert_eq!(ssh_stream_from_worker(&session), "sandbox.ssh.stream.s1.from_worker");
    assert_eq!(SSH_ATTACH_FILTER, "sandbox.ssh.attach.>");
    assert_eq!(SSH_DETACH_FILTER, "sandbox.ssh.detach.>");
    Ok(())
}

#[test]
fn attach_responses_carry_their_outcome() {
    assert!(SshAttachResponse::accepted().ok);
    let rejected = SshAttachResponse::rejected("busy");
    assert!(!rejected.ok);
    assert_eq!(rejected.message.as_deref(), Some("busy"));
    assert_eq!(rejected.refusal, None);
    let unsupported = SshAttachResponse::unsupported("no ssh");
    assert!(!unsupported.ok);
    assert_eq!(unsupported.refusal, Some(SshAttachRefusal::Unsupported));
}

#[test]
fn detach_responses_carry_their_outcome() {
    assert!(SshDetachResponse::accepted().ok);
    let rejected = SshDetachResponse::rejected("unknown");
    assert!(!rejected.ok);
    assert_eq!(rejected.message.as_deref(), Some("unknown"));
}
