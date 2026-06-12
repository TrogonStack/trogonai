use trogonai_session_contracts::SessionId;

/// NATS KV bucket for runtime runner bindings (`ACP_RUNNER_BINDINGS`).
pub fn runner_bindings_bucket(prefix: &str) -> String {
    format!("{prefix}_RUNNER_BINDINGS")
}

/// KV key for a session's active runner binding snapshot.
pub fn runner_binding_key(session_id: &SessionId) -> String {
    format!("sessions.{}.runner_binding", session_id.as_str())
}

/// Request/reply subject for runner turn execution.
pub fn runner_request_subject(prefix: &str, runner_id: &str, session_id: &SessionId) -> String {
    format!(
        "{prefix}.{runner_id}.session.{}.agent.request",
        session_id.as_str()
    )
}

/// Streaming subject for runner provider events.
pub fn runner_stream_subject(prefix: &str, runner_id: &str, session_id: &SessionId) -> String {
    format!(
        "{prefix}.{runner_id}.session.{}.agent.stream",
        session_id.as_str()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nats_names_use_prefix_and_session_id() {
        let session_id = SessionId::new("sess_test").unwrap();
        assert_eq!(runner_bindings_bucket("ACP"), "ACP_RUNNER_BINDINGS");
        assert_eq!(
            runner_binding_key(&session_id),
            "sessions.sess_test.runner_binding"
        );
        assert_eq!(
            runner_request_subject("ACP", "xai", &session_id),
            "ACP.xai.session.sess_test.agent.request"
        );
    }
}
