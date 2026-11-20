use acp_nats::{AcpPrefix, AcpSessionId, nats::responses::CancelledSubject};
use async_nats::subject::ToSubject;

#[test]
fn cancelled_subject_converts_to_canonical_nats_subject() -> Result<(), Box<dyn std::error::Error>> {
    let prefix = AcpPrefix::new("acp")?;
    let session = AcpSessionId::new("session-1")?;

    let subject = CancelledSubject::new(&prefix, &session).to_subject();

    assert_eq!(subject.as_str(), "acp.v1.session.session-1.agent.cancelled");
    Ok(())
}

#[test]
fn cancelled_subject_preserves_distinct_dotted_prefixes() -> Result<(), Box<dyn std::error::Error>> {
    let prefix = AcpPrefix::new("tenant.acp")?;
    let other_prefix = AcpPrefix::new("other.acp")?;
    let session = AcpSessionId::new("session-1")?;

    let subject = CancelledSubject::new(&prefix, &session).to_subject();
    let other_subject = CancelledSubject::new(&other_prefix, &session).to_subject();

    assert_eq!(subject.as_str(), "tenant.acp.v1.session.session-1.agent.cancelled");
    assert_eq!(other_subject.as_str(), "other.acp.v1.session.session-1.agent.cancelled");
    assert_ne!(subject, other_subject);
    Ok(())
}

#[test]
fn cancelled_subject_preserves_distinct_sessions() -> Result<(), Box<dyn std::error::Error>> {
    let prefix = AcpPrefix::new("acp")?;
    let session = AcpSessionId::new("session-1")?;
    let other_session = AcpSessionId::new("session-2")?;

    let subject = CancelledSubject::new(&prefix, &session).to_subject();
    let other_subject = CancelledSubject::new(&prefix, &other_session).to_subject();

    assert_eq!(subject.as_str(), "acp.v1.session.session-1.agent.cancelled");
    assert_eq!(other_subject.as_str(), "acp.v1.session.session-2.agent.cancelled");
    assert_ne!(subject, other_subject);
    Ok(())
}
