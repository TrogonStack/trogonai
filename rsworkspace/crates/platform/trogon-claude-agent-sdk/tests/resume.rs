use trogon_claude_agent_sdk::{ClaudeAgentOptions, SessionId};

#[test]
fn resume_preserves_omission_and_the_callers_opaque_session_id() {
    assert_eq!(ClaudeAgentOptions::default().resume, None);
    let session = SessionId::from("opaque-session");
    let options = ClaudeAgentOptions {
        resume: Some(session.clone()),
        ..ClaudeAgentOptions::default()
    };
    assert_eq!(options.resume.as_ref().map(SessionId::as_str), Some("opaque-session"));
    assert_eq!(options.clone().resume, Some(session));
}
