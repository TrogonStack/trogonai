use trogon_claude_agent_sdk::{ClaudeAgentOptions, SessionId, SessionIdError};

#[test]
fn resume_preserves_omission_and_the_callers_opaque_session_id() -> Result<(), SessionIdError> {
    assert_eq!(ClaudeAgentOptions::default().resume, None);
    let session = SessionId::new("opaque-session")?;
    let options = ClaudeAgentOptions {
        resume: Some(session.clone()),
        ..ClaudeAgentOptions::default()
    };
    assert_eq!(options.resume.as_ref().map(SessionId::as_str), Some("opaque-session"));
    assert_eq!(options.clone().resume, Some(session));
    Ok(())
}
