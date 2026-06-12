use trogonai_session_contracts::SessionId;

/// JetStream stream holding append-only session events.
pub fn session_events_stream(prefix: &str) -> String {
    format!("{prefix}_SESSION_EVENTS")
}

/// NATS KV bucket for materialized session snapshots.
pub fn session_snapshots_bucket(prefix: &str) -> String {
    format!("{prefix}_SESSION_SNAPSHOTS")
}

/// NATS KV bucket for session leases.
pub fn session_leases_bucket(prefix: &str) -> String {
    format!("{prefix}_SESSION_LEASES")
}

/// NATS KV bucket for per-session token usage.
pub fn session_usage_bucket(prefix: &str) -> String {
    format!("{prefix}_SESSION_USAGE")
}

/// JetStream subject for append-only session events.
pub fn session_events_subject(session_id: &SessionId) -> String {
    format!("sessions.{}.events", session_id.as_str())
}

/// KV key for the materialized session snapshot.
pub fn session_snapshot_key(session_id: &SessionId) -> String {
    format!("sessions.{}.state", session_id.as_str())
}

/// KV key for the per-session mutating-operation lease.
pub fn session_lease_key(session_id: &SessionId) -> String {
    format!("sessions.{}.lock", session_id.as_str())
}

/// KV key for the per-session materialized token usage.
pub fn session_usage_key(session_id: &SessionId) -> String {
    format!("sessions.{}.usage", session_id.as_str())
}

/// Wildcard subject filter for all session events in a stream.
pub fn session_events_wildcard() -> &'static str {
    "sessions.*.events"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nats_names_use_prefix_and_session_id() {
        let session_id = SessionId::new("sess_test").unwrap();
        assert_eq!(session_events_stream("ACP"), "ACP_SESSION_EVENTS");
        assert_eq!(session_snapshots_bucket("ACP"), "ACP_SESSION_SNAPSHOTS");
        assert_eq!(session_leases_bucket("ACP"), "ACP_SESSION_LEASES");
        assert_eq!(
            session_events_subject(&session_id),
            "sessions.sess_test.events"
        );
        assert_eq!(
            session_snapshot_key(&session_id),
            "sessions.sess_test.state"
        );
        assert_eq!(session_lease_key(&session_id), "sessions.sess_test.lock");
    }
}
