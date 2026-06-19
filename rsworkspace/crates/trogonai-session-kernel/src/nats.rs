use trogonai_session_contracts::SessionId;

/// NATS resources owned by the Session Kernel rollout for one namespace prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionKernelNamespacePlan {
    pub prefix: String,
    pub event_stream: String,
    pub snapshots_bucket: String,
    pub leases_bucket: String,
    pub usage_bucket: String,
    pub runner_bindings_bucket: String,
    pub context_twin_bucket: String,
    pub artifact_bucket: String,
    pub capability_certifications_bucket: String,
    pub event_subject_wildcard: &'static str,
}

impl SessionKernelNamespacePlan {
    /// Build the environment-specific namespace map required by the NATS operational
    /// policy in `cambio-modelo.md`. The concrete stores are provisioned by their owning
    /// crates, but this gives operators one typed source of truth for streams/buckets and
    /// collision review.
    pub fn for_prefix(prefix: impl Into<String>) -> Self {
        let prefix = prefix.into();
        Self {
            event_stream: session_events_stream(&prefix),
            snapshots_bucket: session_snapshots_bucket(&prefix),
            leases_bucket: session_leases_bucket(&prefix),
            usage_bucket: session_usage_bucket(&prefix),
            runner_bindings_bucket: format!("{prefix}_RUNNER_BINDINGS"),
            context_twin_bucket: format!("{prefix}_CONTEXT_TWIN"),
            artifact_bucket: format!("{prefix}_SESSION_ARTIFACTS"),
            capability_certifications_bucket: format!("{prefix}_CAPABILITY_CERTIFICATIONS"),
            event_subject_wildcard: session_events_wildcard(),
            prefix,
        }
    }
}

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

/// KV key for the per-session routing/migration record (event-log-primary flag and
/// legacy-migration provenance). Stored in the snapshots bucket alongside the snapshot
/// (cambio-modelo.md § "NATS KV para snapshots": `sessions.{id}.routing`).
pub fn session_routing_key(session_id: &SessionId) -> String {
    format!("sessions.{}.routing", session_id.as_str())
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
        assert_eq!(session_events_subject(&session_id), "sessions.sess_test.events");
        assert_eq!(session_snapshot_key(&session_id), "sessions.sess_test.state");
        assert_eq!(session_routing_key(&session_id), "sessions.sess_test.routing");
        assert_eq!(session_lease_key(&session_id), "sessions.sess_test.lock");
    }
}
