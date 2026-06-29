//! Tier-1 SpiceDB denial shaping.
//!
//! When the SpiceDB tier-1 gate denies an ingress request, the
//! dispatch path needs to (a) reply on the caller's inbox with a
//! JSON-RPC `-32801` policy-denied envelope and (b) publish an
//! audit envelope tagged with the denial. Both happen from this
//! module so the orchestrator stays a flat call sequence.
//!
//! Pure boundary: the helper takes a borrowed [`Tier1DenialCtx`] and
//! produces side-effects on the supplied `client`. No state lives
//! here.

use std::time::Instant;

use a2a_nats::A2aPrefix;
use a2a_nats::agent_id::A2aAgentId;
use a2a_nats::audit::envelope::{AuditEnvelope, AuditEnvelopeFields, AuditOutcome, Tier1Decision};
use async_nats::{HeaderMap, Subject};
use bytes::Bytes;
use tracing::warn;
use trogon_nats::PublishClient;

use crate::runtime::audit_publish::spawn_gateway_audit_publish;
use crate::runtime::env::json_rpc_audit_req_id;
use crate::runtime::reply::reply_error;
use crate::runtime::tier1::enrich_audit_caller;

/// All the context the denial flow needs that the dispatch path
/// already has on hand. Borrowed so the caller doesn't pay clones on
/// the hot allow-path -- only the published audit envelope and the
/// detached publish task actually keep owned copies.
pub struct Tier1DenialCtx<'a> {
    pub a2a_prefix: &'a A2aPrefix,
    pub agent_id: &'a A2aAgentId,
    pub method_slashes: &'a str,
    pub payload: &'a Bytes,
    pub trace_id: &'a str,
    pub audit_enabled: bool,
    pub started_wall_ms: u64,
    pub started_mono: Instant,
    pub audit_caller_id: &'a str,
    pub audit_caller_source: &'a Option<String>,
}

/// Reply to the caller with the pre-built JSON-RPC denied envelope
/// `body` and (if `audit_enabled`) publish a matching `-32801` audit
/// envelope. Best-effort on both sides -- a publish failure surfaces
/// only as a warning so other ingress requests stay served.
///
/// The body is built by the caller (typically via
/// `ingress_gateway_policy_denied_response_bytes`) so this helper has
/// no fallible branch to test: the caller decides what to do when
/// the wire helper can't produce a body.
pub async fn deny_tier1<C>(client: &C, reply: Subject, body: Bytes, ctx: Tier1DenialCtx<'_>, message: &str)
where
    C: PublishClient,
{
    warn!(
        agent_id = %ctx.agent_id.as_str(),
        method = %ctx.method_slashes,
        routing_outcome = "tier1_denied",
        "gateway tier-1 SpiceDB denied ingress",
    );
    reply_error(client, reply, HeaderMap::new(), body).await;
    let latency_ms = ctx.started_mono.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    spawn_gateway_audit_publish(
        ctx.audit_enabled,
        client.clone(),
        ctx.a2a_prefix.clone(),
        ctx.agent_id.clone(),
        AuditEnvelope::new(
            ctx.agent_id,
            ctx.method_slashes,
            json_rpc_audit_req_id(ctx.payload.as_ref()),
            ctx.started_wall_ms,
            latency_ms,
            AuditOutcome::Err {
                code: -32_801,
                message: message.into(),
            },
            Some(ctx.payload.as_ref()),
            enrich_audit_caller(
                AuditEnvelopeFields {
                    trace_id: Some(ctx.trace_id.to_owned()),
                    rules_fired: Some(vec!["gateway.tier1.spicedb_denied".into()]),
                    tier1_decision: Some(Tier1Decision::Deny),
                    ..Default::default()
                },
                ctx.audit_caller_id,
                ctx.audit_caller_source,
            ),
        ),
    );
}

#[cfg(test)]
mod tests;
