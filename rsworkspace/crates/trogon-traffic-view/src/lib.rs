//! Agent-traffic view — spec + trait skeleton.
//!
//! See `docs/identity/agent-traffic.md` for schema, projector design, and SIEM format.

use std::fmt;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Normalized audit row written to `agent_traffic_events` (§3).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrafficEvent {
    pub ts: String,
    pub tenant: String,
    pub trace_id: String,
    pub span_id: Option<String>,
    pub source: TrafficSource,
    pub decision: TrafficDecision,
    pub originator_sub: Option<String>,
    pub chain_root: Option<String>,
    pub caller_sub: Option<String>,
    pub agent_id: Option<String>,
    pub callee_agent_id: Option<String>,
    pub callee_aud: Option<String>,
    pub tool_name: Option<String>,
    pub jsonrpc_method: Option<String>,
    pub purpose: Option<String>,
    pub session_id: Option<String>,
    pub wkl: Option<String>,
    pub agent_version: Option<String>,
    pub chain_depth: Option<u32>,
    pub act_chain: Option<Vec<ActChainHop>>,
    pub subject_in: Option<String>,
    pub subject_out: Option<String>,
    pub latency_us: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficSource {
    Gateway,
    Sts,
    Registry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficDecision {
    Allow,
    Deny,
    Rewrite,
    RateLimited,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActChainHop {
    pub sub: String,
    pub agent_id: Option<String>,
    pub wkl: Option<String>,
    pub iat: i64,
}

/// One timeline row (§4).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimelineRow {
    pub ts: String,
    pub caller_agent_id: Option<String>,
    pub caller_sub: Option<String>,
    pub callee: String,
    pub tool: String,
    pub decision: TrafficDecision,
    pub latency_us: Option<u64>,
    pub purpose: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TimelineFilter {
    pub tenant: Option<String>,
    pub agent_id: Option<String>,
    pub originator_sub: Option<String>,
    pub session_id: Option<String>,
    pub window_start: Option<String>,
    pub window_end: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChainNode {
    pub hop_index: u32,
    pub sub: String,
    pub agent_id: Option<String>,
    pub wkl: Option<String>,
    pub iat: Option<i64>,
    pub decision: Option<TrafficDecision>,
    pub tool: Option<String>,
    pub latency_us: Option<u64>,
    pub children: Vec<ChainNode>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChainTree {
    pub trace_id: String,
    pub originator_sub: Option<String>,
    pub roots: Vec<ChainNode>,
}

#[derive(Debug, Error)]
pub enum TrafficViewError {
    #[error("index error: {0}")]
    Index(String),
    #[error("projector error: {0}")]
    Projector(String),
    #[error("siem export error: {0}")]
    Siem(String),
}

pub mod projector {
    use super::*;

    /// JetStream durable consumer name (§9).
    pub const DURABLE_CONSUMER: &str = "agent-traffic-projector";

    #[async_trait]
    pub trait AuditConsumer: Send + Sync {
        /// Run until shutdown; pull from `MCP_AUDIT` and forward to the indexer.
        async fn run(&self) -> Result<(), TrafficViewError>;

        /// Handle one raw audit envelope (JSON bytes, subject included for routing).
        async fn handle_message(&self, subject: &str, payload: &[u8]) -> Result<(), TrafficViewError>;
    }
}

pub mod indexer {
    use super::*;

    #[async_trait]
    pub trait TrafficIndex: Send + Sync {
        async fn put_event(&self, event: TrafficEvent) -> Result<(), TrafficViewError>;

        async fn query_timeline(&self, filter: TimelineFilter) -> Result<Vec<TimelineRow>, TrafficViewError>;

        async fn explore_chain(&self, trace_id: &str) -> Result<ChainTree, TrafficViewError>;
    }
}

pub mod siem {
    use super::*;

    #[async_trait]
    pub trait SiemExporter: Send + Sync {
        async fn export(&self, event: TrafficEvent) -> Result<(), TrafficViewError>;
    }
}

/// Unimplemented projector — see spec §9.
pub struct UnimplementedProjector;

#[async_trait]
impl projector::AuditConsumer for UnimplementedProjector {
    async fn run(&self) -> Result<(), TrafficViewError> {
        todo!("v1 — see docs/identity/agent-traffic.md §9")
    }

    async fn handle_message(&self, _subject: &str, _payload: &[u8]) -> Result<(), TrafficViewError> {
        todo!("v1 — see docs/identity/agent-traffic.md §2")
    }
}

/// Unimplemented Postgres indexer — see spec §3.
pub struct UnimplementedIndex;

#[async_trait]
impl indexer::TrafficIndex for UnimplementedIndex {
    async fn put_event(&self, _event: TrafficEvent) -> Result<(), TrafficViewError> {
        todo!("v1 — see docs/identity/agent-traffic.md §3")
    }

    async fn query_timeline(&self, _filter: TimelineFilter) -> Result<Vec<TimelineRow>, TrafficViewError> {
        todo!("v1 — see docs/identity/agent-traffic.md §4")
    }

    async fn explore_chain(&self, _trace_id: &str) -> Result<ChainTree, TrafficViewError> {
        todo!("v1 — see docs/identity/agent-traffic.md §5")
    }
}

/// Unimplemented OCSF exporter — see spec §7.
pub struct UnimplementedOcsfExporter;

#[async_trait]
impl siem::SiemExporter for UnimplementedOcsfExporter {
    async fn export(&self, _event: TrafficEvent) -> Result<(), TrafficViewError> {
        todo!("v1 — see docs/identity/agent-traffic.md §7")
    }
}

impl fmt::Display for TrafficSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gateway => f.write_str("gateway"),
            Self::Sts => f.write_str("sts"),
            Self::Registry => f.write_str("registry"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traffic_event_serializes() {
        let event = TrafficEvent {
            ts: "2026-05-27T12:00:00Z".into(),
            tenant: "acme".into(),
            trace_id: "trace-1".into(),
            span_id: None,
            source: TrafficSource::Gateway,
            decision: TrafficDecision::Allow,
            originator_sub: Some("user:alice".into()),
            chain_root: Some("user:alice".into()),
            caller_sub: None,
            agent_id: Some("acme/oncall-agent".into()),
            callee_agent_id: None,
            callee_aud: None,
            tool_name: Some("db_query".into()),
            jsonrpc_method: Some("tools/call".into()),
            purpose: Some("incident.response".into()),
            session_id: None,
            wkl: None,
            agent_version: None,
            chain_depth: Some(2),
            act_chain: None,
            subject_in: None,
            subject_out: None,
            latency_us: Some(1_820),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("acme/oncall-agent"));
    }
}
