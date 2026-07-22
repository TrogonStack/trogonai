use trogonai_session_contracts::SessionId;

/// NATS KV bucket for derived Context Twin snapshots.
pub fn context_twins_bucket(prefix: &str) -> String {
    format!("{prefix}_CONTEXT_TWINS")
}

/// KV key for a session's Context Twin.
pub fn context_twin_key(session_id: &SessionId) -> String {
    format!("sessions.{}.context_twin", session_id.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nats_names_use_prefix_and_session_id() {
        let session_id = SessionId::new("sess_test").unwrap();
        assert_eq!(context_twins_bucket("ACP"), "ACP_CONTEXT_TWINS");
        assert_eq!(
            context_twin_key(&session_id),
            "sessions.sess_test.context_twin"
        );
    }
}
