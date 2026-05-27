//! Agent-traffic view — projector, Postgres indexer, and OCSF export.
//!
//! See `docs/identity/agent-traffic.md` for schema, projector design, and SIEM format.

pub mod envelope;
pub mod error;
pub mod event;
pub mod indexer;
pub mod projector;
pub mod query;
pub mod siem;

pub use envelope::AuditEnvelope;
pub use error::{IndexerError, ProjectorError, TrafficViewError};
pub use event::{ActChainHop, TrafficDecision, TrafficEvent, TrafficQueryFilter, TrafficSource};
pub use indexer::postgres::PostgresIndexer;
pub use indexer::TrafficIndex;
pub use projector::consumer::{ensure_audit_stream, JetStreamAuditConsumer};
pub use projector::{
    normalize, AuditConsumer, AuditProjector, ProjectingConsumer, DURABLE_CONSUMER, DEFAULT_STREAM,
    REGISTRY_FILTER, STS_FILTER,
};
pub use query::{parse_since, render_table};
pub use siem::ocsf::{DefaultOcsfExporter, OcsfExporter, emit_ndjson};
pub use siem::{OcsfSiemExporter, SiemExporter};

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    #[test]
    fn traffic_event_serializes() {
        let event = TrafficEvent {
            event_id: "evt-1".into(),
            ts: Utc::now(),
            tenant: "acme".into(),
            caller_sub: Some("acme/oncall-agent".into()),
            caller_wkl: None,
            target_aud: None,
            purpose: Some("incident.response".into()),
            scope: None,
            outcome: "allow".into(),
            reason: None,
            act_chain: None,
            request_id: None,
            session_id: None,
            source: TrafficSource::Gateway,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("acme/oncall-agent"));
    }
}
