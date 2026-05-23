//! Phase 4 — **`a2a-bridge`** end-state runtime planes; HTTPS termination; auth-callout User mint;
//! gateway publish; SSE↔JetStream on **`{prefix}.task.{task_id}.events.>`**; symmetric HTTPS-agent registration.
//!
//! **Status:** compile-only seam — not wired into [`crate::runtime`]. Engineering sketch and deployment
//! guidance: [`../../../../docs/A2A_BRIDGE_SKETCH.md`](../../../../docs/A2A_BRIDGE_SKETCH.md).
//!
//! ## Sibling crate (`a2a-bridge`)
//!
//! The runnable sidecar process will ship from the stub sibling crate at
//! [`../../../../crates/a2a-bridge`](../../../../crates/a2a-bridge) (see its
//! [`BridgeConfig`](../../../../crates/a2a-bridge/src/lib.rs) boundary types). This module documents
//! the **runtime planes** the process wires once HTTP/TLS lands. Compile-only ingress scaffolding also
//! lives in [`super::super::bridge_https_sidecar`].
//!
//! ## End-state process
//!
//! 1. Terminate **HTTPS vanilla A2A** (JSON-RPC over TLS or plain HTTP behind edge TLS).
//! 2. Validate external credentials (OIDC / mTLS / API key) and obtain a **per-request minted NATS User**
//!    via auth-callout ([`super::p0_auth_callout_service::SYS_REQ_USER_AUTH`]).
//! 3. Publish unary RPC on **`{prefix}.gateway.{agent_id}.{method}`** with reply inbox
//!    `_INBOX.{caller_id}.{nonce}`.
//! 4. For streaming methods, attach a JetStream pull consumer on
//!    **`{prefix}.task.{task_id}.events.>`** and map events **SSE↔JetStream** (bidirectional on the
//!    symmetric HTTPS-agent reverse path).
//! 5. [`SymmetricRegistrationPath`] — external HTTPS agents register proxied AgentCards via
//!    `{prefix}.catalog.register.{agent_id}`; gateway `{prefix}.agent.{agent_id}.>` RPC is forwarded
//!    over HTTPS on the reverse path.
//!
//! **No axum/hyper wiring here** — HTTP stack lands in the sibling `a2a-bridge` crate.

use a2a_nats::A2aPrefix;
use a2a_nats::A2aTaskId;
use a2a_nats::nats::subjects::A2aStream;

use super::p0_auth_callout_service::SYS_REQ_USER_AUTH;

/// JetStream stream name for task events inside a tenant Account.
pub const TASK_EVENTS_STREAM: &str = "A2A_EVENTS";

/// NATS system auth subject reused for per-request User minting (see AUTH_CALLOUT sketch §2).
pub const AUTH_CALLOUT_SUBJECT: &str = SYS_REQ_USER_AUTH;

/// Ingress plane for the **`a2a-bridge`** process — HTTPS-facing A2A JSON-RPC termination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngressPlane {
    /// Vanilla A2A JSON-RPC over HTTPS (or plain HTTP when edge proxy terminates TLS).
    HttpsJsonRpcTls,
}

impl IngressPlane {
    /// Short static label for logs and metrics.
    pub const fn label(self) -> &'static str {
        match self {
            Self::HttpsJsonRpcTls => "https_jsonrpc_tls",
        }
    }
}

/// Egress plane for the **`a2a-bridge`** process — JetStream task-event streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressPlane {
    /// Pull consumer on `{prefix}.task.{task_id}.events.>` mapped to HTTPS SSE (and reverse).
    TaskEventJetStream,
}

impl EgressPlane {
    /// Short static label for logs and metrics.
    pub const fn label(self) -> &'static str {
        match self {
            Self::TaskEventJetStream => "task_event_jetstream",
        }
    }
}

/// Runtime planes the end-state **`a2a-bridge`** process wires at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BridgeRuntimePlanes {
    /// HTTPS-facing A2A JSON-RPC termination.
    pub ingress: IngressPlane,
    /// JetStream task-event consumer mapped to SSE egress (and reverse on symmetric agents).
    pub egress: EgressPlane,
}

impl BridgeRuntimePlanes {
    /// Default plane pairing for Phase 4 sidecar deployments.
    pub const fn end_state() -> Self {
        Self {
            ingress: IngressPlane::HttpsJsonRpcTls,
            egress: EgressPlane::TaskEventJetStream,
        }
    }
}

/// Symmetric HTTPS-agent registration — proxied AgentCards and reverse NATS→HTTPS RPC.
///
/// External HTTPS agents register via the catalog registrar; inbound `{prefix}.agent.{agent_id}.>`
/// traffic from the gateway is forwarded over HTTPS on the reverse path
/// ([`A2A_BRIDGE_SKETCH.md`](../../../../docs/A2A_BRIDGE_SKETCH.md) §When `a2a-bridge` fits).
pub trait SymmetricRegistrationPath {
    /// Operator notes for provisioning inbound HTTPS agents (registrar ACL, card schema, reverse RPC).
    fn inbound_https_agent_provision_notes(&self) -> &'static str;
}

/// Per-request NATS User minting via auth-callout before any gateway publish.
///
/// The bridge must **not** substitute a shared bridge User — per-request re-mint preserves caller
/// attribution on gateway spans, audit envelopes, and push DLQ subject segments
/// (BRIDGE sketch §Ingress auth).
pub trait PerRequestNatsUserMint {
    /// Auth-callout system subject the minting client targets.
    fn auth_callout_subject(&self) -> &'static str {
        AUTH_CALLOUT_SUBJECT
    }

    /// Stub — real wiring validates OIDC/mTLS/API-key and returns an Account-bound User JWT.
    fn mint_user_jwt_stub(&self) -> &'static str;
}

/// Gateway publish subject the bridge targets after credential mint.
///
/// Unary A2A methods translate to NATS request/reply on `{prefix}.gateway.{agent_id}.{method}`.
pub trait GatewaySubjectPublish {
    /// Builds `{prefix}.gateway.{agent_id}.{method}` for the minted caller User ACL.
    fn gateway_publish_subject(
        &self,
        prefix: &A2aPrefix,
        agent_id: &str,
        method: &str,
    ) -> String;

    /// Reply inbox token under `_INBOX.{caller_id}.{nonce}` for the minted User subscribe ACL.
    fn reply_inbox_subject(&self, caller_id: &str, nonce: &str) -> String;
}

/// SSE↔JetStream mapping on `{prefix}.task.{task_id}.events.>`.
///
/// Streaming A2A methods attach a JetStream pull consumer on the task-event filter and map frames
/// to HTTPS SSE; the symmetric reverse path adapts external-agent SSE back into JetStream events.
pub trait SseJetStreamTaskEventMap {
    /// JetStream stream backing task events in the tenant Account.
    fn task_events_stream_name(&self, prefix: &A2aPrefix) -> String;

    /// Consumer filter subject — `{prefix}.task.{task_id}.events.>` (NATS `>` tail wildcard).
    fn task_events_filter_subject(&self, prefix: &A2aPrefix, task_id: &A2aTaskId) -> String;

    /// Stub lifecycle hook — real wiring creates an ephemeral pull consumer and SSE encoder loop.
    fn map_jetstream_to_sse_stub(&self) -> &'static str;

    /// Stub lifecycle hook — symmetric HTTPS-agent path: SSE ingress → JetStream publish.
    fn map_sse_to_jetstream_stub(&self) -> &'static str;
}

/// End-state **`a2a-bridge`** process — compile-only aggregate of runtime planes and seams.
pub struct A2aBridgeProcess {
    planes: BridgeRuntimePlanes,
    prefix: String,
}

impl A2aBridgeProcess {
    /// Returns the default end-state plane pairing.
    pub const fn planes(&self) -> BridgeRuntimePlanes {
        self.planes
    }

    /// `{prefix}` inside the tenant Account (default `a2a`).
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Stub lifecycle hook — future `a2a-bridge` binary blocks here after listener + NATS wiring.
    pub fn run_stub(&self) {}
}

impl Default for A2aBridgeProcess {
    fn default() -> Self {
        Self {
            planes: BridgeRuntimePlanes::end_state(),
            prefix: "a2a".to_string(),
        }
    }
}

impl SymmetricRegistrationPath for A2aBridgeProcess {
    fn inbound_https_agent_provision_notes(&self) -> &'static str {
        "Register proxied AgentCards on {prefix}.catalog.register.{agent_id} via the minted \
         caller/registrar User; gateway {prefix}.agent.{agent_id}.> RPC forwards over HTTPS on \
         the reverse path. See docs/A2A_BRIDGE_SKETCH.md and a2a-nats-discovery registrar ACL."
    }
}

impl PerRequestNatsUserMint for A2aBridgeProcess {
    fn mint_user_jwt_stub(&self) -> &'static str {
        "stub-per-request-account-bound-user-jwt"
    }
}

impl GatewaySubjectPublish for A2aBridgeProcess {
    fn gateway_publish_subject(
        &self,
        prefix: &A2aPrefix,
        agent_id: &str,
        method: &str,
    ) -> String {
        format!("{}.gateway.{agent_id}.{method}", prefix.as_str())
    }

    fn reply_inbox_subject(&self, caller_id: &str, nonce: &str) -> String {
        format!("_INBOX.{caller_id}.{nonce}")
    }
}

impl SseJetStreamTaskEventMap for A2aBridgeProcess {
    fn task_events_stream_name(&self, prefix: &A2aPrefix) -> String {
        A2aStream::Events.stream_name(prefix)
    }

    fn task_events_filter_subject(&self, prefix: &A2aPrefix, task_id: &A2aTaskId) -> String {
        format!("{}.task.{task_id}.events.>", prefix.as_str())
    }

    fn map_jetstream_to_sse_stub(&self) -> &'static str {
        "stub-jetstream-pull-to-sse"
    }

    fn map_sse_to_jetstream_stub(&self) -> &'static str {
        "stub-sse-to-jetstream-publish"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefix() -> A2aPrefix {
        A2aPrefix::new("a2a".to_string()).expect("test prefix")
    }

    fn task_id(s: &str) -> A2aTaskId {
        A2aTaskId::new(s).expect("test task id")
    }

    #[test]
    fn end_state_planes_pair_https_ingress_with_jetstream_egress() {
        let planes = BridgeRuntimePlanes::end_state();
        assert_eq!(planes.ingress, IngressPlane::HttpsJsonRpcTls);
        assert_eq!(planes.egress, EgressPlane::TaskEventJetStream);
    }

    #[test]
    fn gateway_publish_subject_matches_sketch() {
        let bridge = A2aBridgeProcess::default();
        let pfx = prefix();
        assert_eq!(
            bridge.gateway_publish_subject(&pfx, "planner", "message.send"),
            "a2a.gateway.planner.message.send"
        );
        assert_eq!(
            bridge.reply_inbox_subject("caller-1", "n1"),
            "_INBOX.caller-1.n1"
        );
    }

    #[test]
    fn task_events_filter_uses_tail_wildcard() {
        let bridge = A2aBridgeProcess::default();
        let pfx = prefix();
        assert_eq!(
            bridge.task_events_filter_subject(&pfx, &task_id("t1")),
            "a2a.task.t1.events.>"
        );
        assert_eq!(bridge.task_events_stream_name(&pfx), TASK_EVENTS_STREAM);
    }

    #[test]
    fn per_request_mint_targets_auth_callout_subject() {
        let bridge = A2aBridgeProcess::default();
        assert_eq!(bridge.auth_callout_subject(), SYS_REQ_USER_AUTH);
        assert!(!bridge.mint_user_jwt_stub().is_empty());
    }

    #[test]
    fn symmetric_registration_notes_reference_registrar_path() {
        let bridge = A2aBridgeProcess::default();
        assert!(bridge
            .inbound_https_agent_provision_notes()
            .contains("catalog.register"));
    }
}
