//! §6 roadmap — Gateway-enforced unary timeout for **`message/send`** style RPC.
//!
//! **Status today:** gateway ingress is an opaque forward. It calls
//! `publish_with_reply_and_headers` toward `{prefix}.agent.{agent_id}.message.send` while leaving
//! the caller's NATS reply inbox (`msg.reply`) untouched. There is no gateway-side correlator
//! waiter, no `tokio::time::timeout`, and no structured JSON-RPC timeout reply from this layer.
//!
//! **Target (§6, [`A2A_PENDING_DECISION.md`](../../../../A2A_PENDING_DECISION.md)):** enforce a
//! **30s** ceiling on unary `message/send` request/reply; longer work must use `message/stream`.
//! Reaching that target requires one of:
//!
//! - **[`UnaryDeadlineMode::ProxiedCorrelationFuture`]** — a gateway **proxy correlator** that owns
//!   the agent-facing reply inbox, waits up to the deadline, and re-publishes (or synthesizes a
//!   timeout error) to the caller inbox; or
//! - **Client enforcement** — callers honor the same deadline on their NATS request/reply client
//!   (`a2a-nats` already defaults `operation_timeout` to 30s) while ingress stays forward-only.
//!
//! This module is compile-time scaffolding only: the §6 contract constant, an explicit mode enum,
//! and a planner trait for stamping span/header metadata ahead of correlator or client wiring.

use std::fmt;
use std::time::Duration;

use async_nats::HeaderMap;
use tracing::Span;

/// Planned NATS header for gateway-propagated unary deadline metadata (not stamped on ingress today).
pub const GATEWAY_UNARY_DEADLINE_SECS_HEADER: &str = "A2A-Gateway-Unary-Deadline-Secs";

/// Planned ingress span field for advertised unary deadline seconds (not wired into runtime yet).
pub const GATEWAY_UNARY_DEADLINE_SECS_SPAN_FIELD: &str = "gateway.unary_deadline_secs";

/// Default **`message/send`** blocking window per §6 ([`A2A_PENDING_DECISION.md`](../../../../A2A_PENDING_DECISION.md)).
///
/// Matches `a2a_nats::constants::DEFAULT_OPERATION_TIMEOUT` (30s). Gateway ingress does **not**
/// enforce this value today; it is the contract constant for a future proxy correlator timeout,
/// ingress audit outcomes, and client-side `operation_timeout` alignment.
pub const DEFAULT_MESSAGE_SEND_DEADLINE_SECS: u64 = 30;

/// How gateway ingress relates to unary deadline enforcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryDeadlineMode {
    /// Current behavior: forward-only relay; caller reply inbox preserved; no gateway deadline.
    ForwardOnlyNoDeadline,
    /// Future behavior: gateway owns agent-side correlation and enforces
    /// [`DEFAULT_MESSAGE_SEND_DEADLINE_SECS`].
    ProxiedCorrelationFuture,
}

impl fmt::Display for UnaryDeadlineMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ForwardOnlyNoDeadline => write!(f, "forward_only_no_deadline"),
            Self::ProxiedCorrelationFuture => write!(f, "proxied_correlation_future"),
        }
    }
}

/// Hooks for tagging ingress observability and forward headers with unary deadline metadata.
///
/// Implementations **must not** start `tokio` timers or block ingress — they only declare intent
/// (mode, deadline seconds) on spans and optional NATS headers before the opaque forward publish.
pub trait UnaryDeadlinePlanner {
    /// Active deadline story for this ingress dispatch.
    fn unary_deadline_mode(&self) -> UnaryDeadlineMode;

    /// Deadline seconds to advertise on spans/headers (typically [`DEFAULT_MESSAGE_SEND_DEADLINE_SECS`]).
    fn message_send_deadline_secs(&self) -> u64;

    /// Record deadline metadata on the ingress dispatch span.
    fn tag_ingress_span(&self, span: &Span) {
        span.record(
            GATEWAY_UNARY_DEADLINE_SECS_SPAN_FIELD,
            self.message_send_deadline_secs(),
        );
        span.record(
            "gateway.unary_deadline_mode",
            tracing::field::display(self.unary_deadline_mode()),
        );
    }

    /// Stamp forward headers with deadline metadata when the active mode calls for propagation.
    ///
    /// Default is a no-op so [`UnaryDeadlineMode::ForwardOnlyNoDeadline`] keeps today's bit-identical
    /// relay (caller reply inbox and header set unchanged aside from ingress routing).
    fn stamp_forward_headers(&self, headers: &mut HeaderMap) {
        if self.unary_deadline_mode() == UnaryDeadlineMode::ProxiedCorrelationFuture {
            headers.insert(
                GATEWAY_UNARY_DEADLINE_SECS_HEADER,
                self.message_send_deadline_secs().to_string(),
            );
        }
    }

    /// [`DEFAULT_MESSAGE_SEND_DEADLINE_SECS`] as a `Duration` for planners that need a typed value.
    fn default_message_send_deadline(&self) -> Duration {
        Duration::from_secs(DEFAULT_MESSAGE_SEND_DEADLINE_SECS)
    }
}
