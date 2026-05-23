//! Phase 2 — **`message/send`** 30s gateway deadline; long work → **`message/stream`**.
//!
//! **Status today:** gateway ingress is an opaque forward toward
//! `{prefix}.agent.{agent_id}.message.send` with the caller NATS reply inbox preserved. There is no
//! gateway-side correlator waiter, no `tokio::time::timeout`, and no structured JSON-RPC timeout
//! reply from this layer.
//!
//! **Target ([`A2A_PENDING_DECISION.md`](../../../../A2A_PENDING_DECISION.md) §6):** enforce a
//! **30s** ceiling on unary **`message/send`** request/reply at the gateway. Work that may exceed
//! that window must use **`message/stream`** instead — callers that block on unary `message/send`
//! past the deadline should receive a gateway timeout error and transition to streaming.
//!
//! ## Enforcement variants
//!
//! Reaching the §6 contract requires one of two stories (both honor
//! [`MESSAGE_SEND_GATEWAY_DEADLINE_SECS`]):
//!
//! - **[`UnaryDeadlineEnforcement::DualHopCorrelator`]** — gateway **proxy correlator** (dual-hop):
//!   ingress owns the agent-facing reply inbox, waits up to the deadline, then re-publishes the
//!   agent reply (or synthesizes a JSON-RPC timeout) to the caller inbox. The caller sees one
//!   request/reply hop; the gateway performs a second correlated hop toward the agent.
//! - **[`UnaryDeadlineEnforcement::ClientEnforcement`]** — ingress stays forward-only; callers honor
//!   the same deadline on their NATS request/reply client (`a2a_nats::constants::DEFAULT_OPERATION_TIMEOUT`
//!   is already 30s). Gateway [`P2UnaryDeadlineRouter`] classification still tags spans/audit for
//!   observability alignment even when no proxy correlator runs.
//!
//! This module is compile-time scaffolding only: the §6 contract constant, method classification,
//! and planner hooks ahead of correlator wiring or client-side timeout alignment.

use std::fmt;
use std::time::Duration;

use async_nats::HeaderMap;
use tracing::Span;

/// Planned NATS header for gateway-propagated unary deadline metadata (not stamped on ingress today).
pub const GATEWAY_UNARY_DEADLINE_SECS_HEADER: &str = "A2A-Gateway-Unary-Deadline-Secs";

/// Planned ingress span field for advertised unary deadline seconds (not wired into runtime yet).
pub const GATEWAY_UNARY_DEADLINE_SECS_SPAN_FIELD: &str = "gateway.unary_deadline_secs";

/// Gateway-enforced blocking window for unary **`message/send`** per §6
/// ([`A2A_PENDING_DECISION.md`](../../../../A2A_PENDING_DECISION.md)).
///
/// Matches `a2a_nats::constants::DEFAULT_OPERATION_TIMEOUT` (30s). Ingress does **not** enforce
/// this value today; it is the contract constant for a future proxy correlator timeout, ingress
/// audit outcomes, and client-side `operation_timeout` alignment.
pub const MESSAGE_SEND_GATEWAY_DEADLINE_SECS: u64 = 30;

/// How gateway ingress relates to unary deadline enforcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnaryDeadlineEnforcement {
    /// Forward-only relay today; caller reply inbox preserved; deadline honored by the client SDK.
    #[default]
    ClientEnforcement,
    /// Future behavior: gateway owns agent-side correlation (dual-hop) and enforces
    /// [`MESSAGE_SEND_GATEWAY_DEADLINE_SECS`].
    DualHopCorrelator,
}

impl fmt::Display for UnaryDeadlineEnforcement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClientEnforcement => write!(f, "client_enforcement"),
            Self::DualHopCorrelator => write!(f, "dual_hop_correlator"),
        }
    }
}

/// Ingress routing class derived from the A2A method suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadlineClass {
    /// Unary RPC subject to the [`MESSAGE_SEND_GATEWAY_DEADLINE_SECS`] ceiling.
    UnaryThirtySecs,
    /// Streaming path for work that may exceed the unary ceiling.
    StreamingOnly,
}

impl fmt::Display for DeadlineClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnaryThirtySecs => write!(f, "unary_thirty_secs"),
            Self::StreamingOnly => write!(f, "streaming_only"),
        }
    }
}

impl DeadlineClass {
    /// Deadline seconds advertised for this class (`None` when streaming owns the lifecycle).
    pub fn deadline_secs(self) -> Option<u64> {
        match self {
            Self::UnaryThirtySecs => Some(MESSAGE_SEND_GATEWAY_DEADLINE_SECS),
            Self::StreamingOnly => None,
        }
    }
}

/// Classifies ingress methods into unary vs streaming deadline buckets.
pub trait P2UnaryDeadlineRouter {
    /// Map a JSON-RPC method (`message/send`) or dotted NATS suffix (`message.send`) to a deadline class.
    fn classify_method(method: &str) -> DeadlineClass;

    /// Active enforcement story for this ingress dispatch.
    fn unary_deadline_enforcement(&self) -> UnaryDeadlineEnforcement;

    /// Record deadline metadata on the ingress dispatch span.
    fn tag_ingress_span(&self, span: &Span, method: &str) {
        let class = Self::classify_method(method);
        span.record(
            GATEWAY_UNARY_DEADLINE_SECS_SPAN_FIELD,
            class.deadline_secs(),
        );
        span.record("gateway.deadline_class", tracing::field::display(class));
        span.record(
            "gateway.unary_deadline_enforcement",
            tracing::field::display(self.unary_deadline_enforcement()),
        );
    }

    /// Stamp forward headers with deadline metadata when dual-hop correlator mode is active.
    ///
    /// Default is a no-op so [`UnaryDeadlineEnforcement::ClientEnforcement`] keeps today's
    /// bit-identical relay (caller reply inbox and header set unchanged aside from ingress routing).
    fn stamp_forward_headers(&self, headers: &mut HeaderMap, method: &str) {
        if self.unary_deadline_enforcement() != UnaryDeadlineEnforcement::DualHopCorrelator {
            return;
        }
        if let Some(deadline_secs) = Self::classify_method(method).deadline_secs() {
            headers.insert(
                GATEWAY_UNARY_DEADLINE_SECS_HEADER,
                deadline_secs.to_string(),
            );
        }
    }

    /// [`MESSAGE_SEND_GATEWAY_DEADLINE_SECS`] as a `Duration` for planners that need a typed value.
    fn message_send_gateway_deadline(&self) -> Duration {
        Duration::from_secs(MESSAGE_SEND_GATEWAY_DEADLINE_SECS)
    }
}

/// Default Phase 2 router: **`message/send`** → unary 30s; **`message/stream`** → streaming-only.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultP2UnaryDeadlineRouter {
    enforcement: UnaryDeadlineEnforcement,
}

impl DefaultP2UnaryDeadlineRouter {
    pub const fn new(enforcement: UnaryDeadlineEnforcement) -> Self {
        Self { enforcement }
    }

    pub const fn client_enforcement() -> Self {
        Self::new(UnaryDeadlineEnforcement::ClientEnforcement)
    }

    pub const fn dual_hop_correlator() -> Self {
        Self::new(UnaryDeadlineEnforcement::DualHopCorrelator)
    }
}

impl P2UnaryDeadlineRouter for DefaultP2UnaryDeadlineRouter {
    fn classify_method(method: &str) -> DeadlineClass {
        match normalize_method(method) {
            "message.send" | "message/send" => DeadlineClass::UnaryThirtySecs,
            "message.stream" | "message/stream" => DeadlineClass::StreamingOnly,
            _ => DeadlineClass::UnaryThirtySecs,
        }
    }

    fn unary_deadline_enforcement(&self) -> UnaryDeadlineEnforcement {
        self.enforcement
    }
}

fn normalize_method(method: &str) -> &str {
    method.trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_send_deadline_constant_matches_contract() {
        assert_eq!(MESSAGE_SEND_GATEWAY_DEADLINE_SECS, 30);
    }

    #[test]
    fn classify_message_send_as_unary_thirty_secs() {
        assert_eq!(
            DefaultP2UnaryDeadlineRouter::classify_method("message/send"),
            DeadlineClass::UnaryThirtySecs
        );
        assert_eq!(
            DefaultP2UnaryDeadlineRouter::classify_method("message.send"),
            DeadlineClass::UnaryThirtySecs
        );
    }

    #[test]
    fn classify_message_stream_as_streaming_only() {
        assert_eq!(
            DefaultP2UnaryDeadlineRouter::classify_method("message/stream"),
            DeadlineClass::StreamingOnly
        );
        assert_eq!(
            DefaultP2UnaryDeadlineRouter::classify_method("message.stream"),
            DeadlineClass::StreamingOnly
        );
    }

    #[test]
    fn deadline_secs_only_on_unary() {
        assert_eq!(
            DeadlineClass::UnaryThirtySecs.deadline_secs(),
            Some(MESSAGE_SEND_GATEWAY_DEADLINE_SECS)
        );
        assert_eq!(DeadlineClass::StreamingOnly.deadline_secs(), None);
    }
}
