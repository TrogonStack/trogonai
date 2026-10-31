//! `{prefix}.v1.gateway.{agent}.{kind}.{id}.{method}` ingress dispatch orchestrator.
//!
//! Runs each ingress request through the full policy stack in
//! order: forwarded caller JWT check -> Tier-1 SpiceDB ->
//! Tier-1 declarative -> Tier-2 CEL -> Tier-3 redaction. Each
//! denial path replies on the caller inbox with the matching
//! JSON-RPC error code and publishes an audit envelope; the
//! allow path forwards to `{prefix}.v1.agents.{agent}.{kind}.{id}.{method}`
//! and (when configured) spawns a streaming pump for stream-shaped
//! methods. A subject that names no caller never reaches a tier. Every
//! tier, audit and forward sees the principal the ingress subject carried
//! and no other, so a header or payload can never re-attribute a relayed
//! request (ADR#0082).

#![cfg(feature = "spicedb")]

use std::sync::Arc;
use std::time::Instant;

use a2a_nats::A2aMethod;
use a2a_nats::audit::envelope::{AuditEnvelope, AuditEnvelopeFields, AuditOutcome, Tier1Decision, Tier3Decision};
use a2a_nats::nats::subjects::gateway::GatewayIngressSubject;
use a2a_nats::{
    ingress_gateway_deadline_exceeded_response_bytes, ingress_gateway_declarative_denied_response_bytes,
    ingress_gateway_policy_denied_response_bytes, ingress_gateway_tier3_refused_response_bytes,
    ingress_invalid_request_response_bytes,
};
use async_nats::HeaderMap;
use bytes::Bytes;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, error, info, warn};
use trogon_identity_types::SubjectPrincipal;
use trogon_std::env::ReadEnv;
use uuid::Uuid;

use crate::config::Config;
use crate::gw_ingress_stream::{
    CallerKey, GatewayStreamingIngressConfig, StreamingIngressGate, events_stream_last_seq,
};
use crate::ingress_identity::{ForwardedCallerJwtVerifier, without_identity_headers};
use crate::policy::CallerSpiceDbSubject;
use crate::policy::spicedb_tier1::{
    OwnerTupleEmitter, SpiceDbTier1Gate, Tier1AuthorizeOutcome, Tier1OwnerTuple, derive_tuple,
    tier1_principal_from_caller, tier1_session_from_caller,
};
use crate::policy::tier1_declarative::{
    Tier1DeclarativeDecision, Tier1DeclarativeGate, tier1_declarative_audit_rule_fired,
};
use crate::policy::tier2::Tier2Decision;
use crate::policy::tier2_cel::tier2_evaluation_context_from_ingress;
use crate::policy::tier3_redaction::{
    Tier3EvaluationContext, Tier3RedactionDecision, gateway_tier3_redaction_enabled, merge_forward_audit_rewrites,
    tier3_redaction_audit_rewrites,
};
use crate::runtime::audit_publish::spawn_gateway_audit_publish;
use crate::runtime::env::{
    gateway_audit_publish_enabled, json_rpc_audit_req_id, json_rpc_params, unary_deadline_for_method, unix_epoch_ms,
};
use crate::runtime::policy_stack::GatewayPolicyStack;
use crate::runtime::reply::reply_error;
use crate::runtime::streaming::maybe_spawn_streaming_ingress_pump;
use crate::runtime::tier1::{enrich_audit_caller, tier1_declarative_context_from_ingress};
use crate::runtime::tier1_denial::{Tier1DenialCtx, deny_tier1};

use crate::constants::MESSAGE_STREAM_METHOD_DOTS;
use crate::constants::{
    ATTR_AGENT_SUBJECT, ATTR_CALLER_ID, ATTR_ROUTING_OUTCOME, ENV_GATEWAY_JWT_AUDIENCE, ROUTING_CALLER_JWT_REJECTED,
    ROUTING_DEADLINE_EXCEEDED, ROUTING_FORWARD_FAILED, ROUTING_FORWARDED, ROUTING_IGNORED_NO_REPLY,
    ROUTING_INGRESS_ERROR, ROUTING_POLICY_DENIED, ROUTING_TIER1_DENIED, ROUTING_TIER3_ENGINE_ERROR,
    ROUTING_TIER3_REFUSED, SPAN_GATEWAY_INGRESS_DISPATCH,
};

/// Run a single ingress envelope through the full gateway dispatch
/// chain. Returns once the reply (or detached audit publish) has
/// been initiated; the caller is responsible for spawning this
/// future on the request-handling task pool.
///
/// `tier1_owner` is `None` on deployments that don't write
/// owner-relationship tuples back to SpiceDB; passing it as
/// `Option<&Arc<...>>` keeps the dispatch path from holding an
/// extra reference cycle on the gate that owns the emitter.
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_gateway_ingress<E: ReadEnv>(
    client: &async_nats::Client,
    config: &Config,
    tier1: &dyn SpiceDbTier1Gate,
    tier1_owner: Option<&Arc<dyn OwnerTupleEmitter>>,
    tier1_declarative: &dyn Tier1DeclarativeGate,
    policy: &GatewayPolicyStack,
    caller_jwt: &ForwardedCallerJwtVerifier,
    streaming_ingress_enabled: bool,
    streaming_ingress_config: GatewayStreamingIngressConfig,
    streaming_ingress_gate: &StreamingIngressGate,
    shutdown: CancellationToken,
    env: &E,
    msg: async_nats::Message,
) {
    let ingress_subject = msg.subject.as_str().to_owned();
    let reply_present = msg.reply.is_some();

    let span = tracing::info_span!(
        SPAN_GATEWAY_INGRESS_DISPATCH,
        gateway_ingress.subject = %ingress_subject,
        ingress.reply_present = reply_present,
        caller_id = tracing::field::Empty,
        agent_subject = tracing::field::Empty,
        routing_outcome = tracing::field::Empty,
    );

    async {
        debug!(
            ingress.subject = %ingress_subject,
            ingress.reply_present = reply_present,
            payload_len = msg.payload.len(),
            "gateway ingress envelope received",
        );

        let Some(reply) = msg.reply.clone() else {
            tracing::Span::current().record(ATTR_ROUTING_OUTCOME, ROUTING_IGNORED_NO_REPLY);
            warn!(
                ingress.subject = %ingress_subject,
                ingress.reply_present = false,
                routing_outcome = "ignored_no_reply",
                "gateway ingress without reply inbox; ignoring",
            );
            return;
        };

        match GatewayIngressSubject::parse(&config.a2a_prefix, ingress_subject.as_str()) {
            Ok(ingress) => {
                dispatch_routed(
                    client,
                    config,
                    tier1,
                    tier1_owner,
                    tier1_declarative,
                    policy,
                    caller_jwt,
                    streaming_ingress_enabled,
                    streaming_ingress_config,
                    streaming_ingress_gate,
                    shutdown,
                    env,
                    &msg,
                    reply,
                    ingress_subject.as_str(),
                    ingress,
                )
                .await;
            }
            Err(reason) => {
                handle_ingress_routing_failure(client, &msg, reply, reason.to_string()).await;
            }
        }
    }
    .instrument(span)
    .await
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_routed<E: ReadEnv>(
    client: &async_nats::Client,
    config: &Config,
    tier1: &dyn SpiceDbTier1Gate,
    tier1_owner: Option<&Arc<dyn OwnerTupleEmitter>>,
    tier1_declarative: &dyn Tier1DeclarativeGate,
    policy: &GatewayPolicyStack,
    caller_jwt: &ForwardedCallerJwtVerifier,
    streaming_ingress_enabled: bool,
    streaming_ingress_config: GatewayStreamingIngressConfig,
    streaming_ingress_gate: &StreamingIngressGate,
    shutdown: CancellationToken,
    env: &E,
    msg: &async_nats::Message,
    reply: async_nats::Subject,
    ingress_subject: &str,
    ingress: GatewayIngressSubject,
) {
    let caller = ingress.caller().clone();
    tracing::Span::current().record(ATTR_CALLER_ID, tracing::field::display(&caller));
    let agent_id = ingress.agent_id().clone();
    let method = ingress.method().clone();
    let method_dots = method.as_dotted_suffix().to_owned();
    let agent_subject = ingress.agent_delivery().to_string();
    let inbound_headers = msg.headers.clone().unwrap_or_default();
    tracing::Span::current().record(ATTR_AGENT_SUBJECT, tracing::field::display(&agent_subject));

    let mut payload: Bytes = msg.payload.clone();
    let started_mono = Instant::now();
    let started_wall_ms = unix_epoch_ms();
    let trace_id = Uuid::now_v7().to_string();
    let audit_enabled = gateway_audit_publish_enabled(env);
    let method_slashes = method_dots.replace('.', "/");
    let _unary_deadline_guard = unary_deadline_for_method(env, method_dots.as_str());

    if let Err(rejection) = caller_jwt.check(&inbound_headers, &caller) {
        tracing::Span::current().record(ATTR_ROUTING_OUTCOME, ROUTING_CALLER_JWT_REJECTED);
        warn!(
            ingress.subject = %ingress_subject,
            caller_id = %caller,
            rule = rejection.audit_rule(),
            reason = %rejection,
            routing_outcome = "caller_jwt_rejected",
            "gateway rejected a forwarded caller JWT that does not prove the subject principal",
        );
        let body =
            ingress_invalid_request_response_bytes(&inbound_headers, payload.as_ref(), "forwarded caller JWT rejected");
        reply_error(client, reply, HeaderMap::new(), body).await;
        spawn_gateway_audit_publish(
            audit_enabled,
            client.clone(),
            config.a2a_prefix.clone(),
            agent_id.clone(),
            AuditEnvelope::new(
                &agent_id,
                method_slashes.clone(),
                json_rpc_audit_req_id(payload.as_ref()),
                started_wall_ms,
                elapsed_ms(started_mono),
                AuditOutcome::Err {
                    code: -32_600,
                    message: "forwarded caller JWT rejected".into(),
                },
                Some(payload.as_ref()),
                enrich_audit_caller(
                    AuditEnvelopeFields {
                        trace_id: Some(trace_id.clone()),
                        rules_fired: Some(vec![rejection.audit_rule().into()]),
                        ..Default::default()
                    },
                    &caller,
                ),
            ),
        );
        return;
    }

    let headers_owned = without_identity_headers(&inbound_headers);
    let caller_spicedb = CallerSpiceDbSubject::from(&caller);
    let mut tier1_zed_token: Option<String> = None;

    if tier1.is_enabled() {
        match run_tier1_spicedb(
            client,
            reply.clone(),
            config,
            tier1,
            tier1_owner,
            &agent_id,
            &method,
            method_slashes.as_str(),
            &headers_owned,
            payload.as_ref(),
            &caller,
            &caller_spicedb,
            audit_enabled,
            started_wall_ms,
            started_mono,
            trace_id.as_str(),
            env,
        )
        .await
        {
            Tier1SpiceDbStepOutcome::Allow { zed_token } => {
                tier1_zed_token = zed_token;
            }
            Tier1SpiceDbStepOutcome::Denied => {
                tracing::Span::current().record(ATTR_ROUTING_OUTCOME, ROUTING_TIER1_DENIED);
                return;
            }
        }
    }

    let mut _tier1_declarative_audit: Option<String> = None;
    if tier1_declarative.is_enabled()
        && let Some(declarative_ctx) =
            tier1_declarative_context_from_ingress(method_dots.as_str(), &agent_id, &caller_spicedb, ingress_subject)
    {
        let decision = tier1_declarative.evaluate(&declarative_ctx);
        _tier1_declarative_audit = Some(tier1_declarative_audit_rule_fired(&decision));
        if let Tier1DeclarativeDecision::Deny { rule } = decision {
            tracing::Span::current().record(ATTR_ROUTING_OUTCOME, ROUTING_POLICY_DENIED);
            warn!(
                ingress.subject = %ingress_subject,
                agent_subject = %agent_subject,
                rule = %rule,
                routing_outcome = "policy_denied",
                "gateway tier-1 declarative policy rejected ingress envelope",
            );
            let body = ingress_gateway_declarative_denied_response_bytes(
                &headers_owned,
                payload.as_ref(),
                "tier-1 declarative policy rejected envelope",
            );
            reply_error(client, reply, HeaderMap::new(), body).await;
            spawn_gateway_audit_publish(
                audit_enabled,
                client.clone(),
                config.a2a_prefix.clone(),
                agent_id.clone(),
                AuditEnvelope::new(
                    &agent_id,
                    method_slashes.clone(),
                    json_rpc_audit_req_id(payload.as_ref()),
                    started_wall_ms,
                    elapsed_ms(started_mono),
                    AuditOutcome::Err {
                        code: -32_803,
                        message: "tier-1 declarative policy rejected envelope".into(),
                    },
                    Some(payload.as_ref()),
                    enrich_audit_caller(
                        AuditEnvelopeFields {
                            trace_id: Some(trace_id.clone()),
                            rules_fired: Some(vec![format!("gateway.tier1.declarative.denied.{}", rule.as_str())]),
                            tier1_decision: Some(Tier1Decision::Deny),
                            zed_token_snapshot: tier1_zed_token.clone(),
                            ..Default::default()
                        },
                        &caller,
                    ),
                ),
            );
            return;
        }
    }

    if let Some(substrate) = policy.substrate.as_ref()
        && let Some(evaluator) = substrate.tier2.evaluator()
    {
        let caller_subject = caller_spicedb.to_spicedb_subject();
        let eval_ctx = tier2_evaluation_context_from_ingress(
            method,
            &agent_id,
            Some(&caller_subject),
            &headers_owned,
            payload.as_ref(),
        );
        match evaluator.evaluate(&eval_ctx) {
            Tier2Decision::Allow => {}
            Tier2Decision::Deny { rule } => {
                tracing::Span::current().record(ATTR_ROUTING_OUTCOME, ROUTING_POLICY_DENIED);
                warn!(
                    ingress.subject = %ingress_subject,
                    agent_subject = %agent_subject,
                    rule = %rule,
                    routing_outcome = "policy_denied",
                    "gateway tier-2 predicate rejected ingress envelope",
                );
                let body = ingress_gateway_policy_denied_response_bytes(
                    &headers_owned,
                    payload.as_ref(),
                    "tier-2 predicate rejected envelope",
                );
                reply_error(client, reply, HeaderMap::new(), body).await;
                spawn_gateway_audit_publish(
                    audit_enabled,
                    client.clone(),
                    config.a2a_prefix.clone(),
                    agent_id.clone(),
                    AuditEnvelope::new(
                        &agent_id,
                        method_slashes.clone(),
                        json_rpc_audit_req_id(payload.as_ref()),
                        started_wall_ms,
                        elapsed_ms(started_mono),
                        AuditOutcome::Err {
                            code: -32_801,
                            message: "tier-2 predicate rejected envelope".into(),
                        },
                        Some(payload.as_ref()),
                        enrich_audit_caller(
                            AuditEnvelopeFields {
                                trace_id: Some(trace_id.clone()),
                                rules_fired: Some(vec![format!("gateway.tier2.{}", rule.as_str())]),
                                ..Default::default()
                            },
                            &caller,
                        ),
                    ),
                );
                return;
            }
        }
    }

    let mut tier3_rewrites = Vec::new();
    let mut tier3_ctx = match Tier3EvaluationContext::from_json_rpc_payload(
        method_slashes.clone(),
        Some(caller.to_string()),
        payload.as_ref(),
        policy.tier3_manifests.clone(),
    ) {
        Ok(ctx) => ctx,
        Err(_parse_err) => {
            // Unparseable payload -- fail closed rather than fall
            // through to a Null-payload manifest miss. Audit + reply
            // share the same -32801 shape as a policy denial.
            let body = ingress_gateway_policy_denied_response_bytes(
                &headers_owned,
                payload.as_ref(),
                "tier-3 ingress payload not valid JSON",
            );
            reply_error(client, reply, HeaderMap::new(), body).await;
            return;
        }
    };
    match policy.tier3_gate.redact(&mut tier3_ctx) {
        Tier3RedactionDecision::Allow { rewrites } => {
            tier3_rewrites = rewrites;
            if !tier3_rewrites.is_empty() {
                info!(
                    count = tier3_rewrites.len(),
                    caller_id = %caller,
                    method = %method_slashes,
                    "gateway tier-3 redaction applied before forward",
                );
            }
            payload = tier3_ctx.into_payload_bytes();
        }
        Tier3RedactionDecision::Refuse { reason, rule } => {
            tracing::Span::current().record(ATTR_ROUTING_OUTCOME, ROUTING_TIER3_REFUSED);
            warn!(
                skill_id = %rule,
                caller_id = %caller,
                method = %method_slashes,
                reason = %reason.as_str(),
                routing_outcome = "tier3_refused",
                "gateway tier-3 skill refused part redaction",
            );
            let body = ingress_gateway_tier3_refused_response_bytes(
                &headers_owned,
                payload.as_ref(),
                "tier-3 skill refused part redaction",
                rule.as_str(),
            );
            reply_error(client, reply, HeaderMap::new(), body).await;
            spawn_gateway_audit_publish(
                audit_enabled,
                client.clone(),
                config.a2a_prefix.clone(),
                agent_id.clone(),
                AuditEnvelope::new(
                    &agent_id,
                    method_slashes.clone(),
                    json_rpc_audit_req_id(payload.as_ref()),
                    started_wall_ms,
                    elapsed_ms(started_mono),
                    AuditOutcome::Err {
                        code: -32_802,
                        message: format!("tier-3 skill refused: {}", reason.as_str()),
                    },
                    Some(payload.as_ref()),
                    enrich_audit_caller(
                        AuditEnvelopeFields {
                            trace_id: Some(trace_id.clone()),
                            rules_fired: Some(vec![format!("gateway.tier3.refused.{}", rule.as_str())]),
                            rewrites: tier3_redaction_audit_rewrites(&tier3_rewrites),
                            tier3_decision: Some(Tier3Decision::Refuse),
                            ..Default::default()
                        },
                        &caller,
                    ),
                ),
            );
            return;
        }
        Tier3RedactionDecision::Error { rule, kind } => {
            tracing::Span::current().record(ATTR_ROUTING_OUTCOME, ROUTING_TIER3_ENGINE_ERROR);
            error!(
                skill_id = %rule,
                caller_id = %caller,
                method = %method_slashes,
                kind = %kind.as_str(),
                routing_outcome = "tier3_engine_error",
                "gateway tier-3 redaction engine failed closed",
            );
            let body = ingress_gateway_policy_denied_response_bytes(
                &headers_owned,
                payload.as_ref(),
                "tier-3 redaction engine error",
            );
            reply_error(client, reply, HeaderMap::new(), body).await;
            spawn_gateway_audit_publish(
                audit_enabled,
                client.clone(),
                config.a2a_prefix.clone(),
                agent_id.clone(),
                AuditEnvelope::new(
                    &agent_id,
                    method_slashes.clone(),
                    json_rpc_audit_req_id(payload.as_ref()),
                    started_wall_ms,
                    elapsed_ms(started_mono),
                    AuditOutcome::Err {
                        code: -32_801,
                        message: "tier-3 redaction engine error".into(),
                    },
                    Some(payload.as_ref()),
                    enrich_audit_caller(
                        AuditEnvelopeFields {
                            trace_id: Some(trace_id.clone()),
                            rules_fired: Some(vec!["gateway.tier3.engine_error".into()]),
                            rewrites: tier3_redaction_audit_rewrites(&tier3_rewrites),
                            tier3_decision: Some(Tier3Decision::Error),
                            ..Default::default()
                        },
                        &caller,
                    ),
                ),
            );
            return;
        }
    }

    debug!(
        ingress.subject = %ingress_subject,
        agent_subject = %agent_subject,
        ingress.reply_present = true,
        reply = %reply,
        "gateway forwarding to agent subject",
    );

    // Anchor the streaming pump before the agent can publish anything: the pump is
    // spawned after the forward and creates its consumer asynchronously, so the
    // start position has to be read from this side of the publish.
    let events_last_seq = if streaming_ingress_enabled && method_dots.as_str() == MESSAGE_STREAM_METHOD_DOTS {
        events_stream_last_seq(client, &config.a2a_prefix).await
    } else {
        None
    };

    let disposition = forward_to_agent(
        client,
        env,
        agent_subject.as_str(),
        method_dots.as_str(),
        reply.clone(),
        headers_owned.clone(),
        payload.clone(),
    )
    .await;

    let rules_fired = build_rules_fired(tier1, policy, &tier3_rewrites, env);
    let (rewrites, stream_consumer) = merge_forward_audit_rewrites(
        &tier3_rewrites,
        ingress_subject,
        &agent_subject,
        &agent_id,
        method_dots.as_str(),
    );

    match disposition {
        ForwardDisposition::Ok => {
            tracing::Span::current().record(ATTR_ROUTING_OUTCOME, ROUTING_FORWARDED);
            if streaming_ingress_enabled && let Ok(caller_key) = CallerKey::new(caller.to_string()) {
                let _ = maybe_spawn_streaming_ingress_pump(
                    client,
                    &config.a2a_prefix,
                    streaming_ingress_config,
                    streaming_ingress_gate,
                    shutdown.clone(),
                    method_dots.as_str(),
                    &headers_owned,
                    payload.as_ref(),
                    reply.clone(),
                    caller_key,
                    caller.clone(),
                    events_last_seq,
                );
            }
            spawn_gateway_audit_publish(
                audit_enabled,
                client.clone(),
                config.a2a_prefix.clone(),
                agent_id.clone(),
                AuditEnvelope::new(
                    &agent_id,
                    method_slashes.clone(),
                    json_rpc_audit_req_id(payload.as_ref()),
                    started_wall_ms,
                    elapsed_ms(started_mono),
                    AuditOutcome::Ok,
                    Some(payload.as_ref()),
                    enrich_audit_caller(
                        AuditEnvelopeFields {
                            trace_id: Some(trace_id.clone()),
                            rules_fired: Some(rules_fired),
                            rewrites,
                            stream_consumer,
                            zed_token_snapshot: tier1_zed_token.clone(),
                            tier1_decision: tier1.is_enabled().then_some(Tier1Decision::Allow),
                            tier3_decision: gateway_tier3_redaction_enabled(env).then_some(Tier3Decision::Allow),
                            ..Default::default()
                        },
                        &caller,
                    ),
                ),
            );
        }
        ForwardDisposition::Publish(error) => {
            tracing::Span::current().record(ATTR_ROUTING_OUTCOME, ROUTING_FORWARD_FAILED);
            warn!(
                ingress.subject = %ingress_subject,
                agent_subject = %agent_subject,
                ingress.reply_present = true,
                routing_outcome = "forward_failed",
                error = %error,
                "gateway failed to publish forward to agent subject",
            );
            spawn_gateway_audit_publish(
                audit_enabled,
                client.clone(),
                config.a2a_prefix.clone(),
                agent_id.clone(),
                AuditEnvelope::new(
                    &agent_id,
                    method_slashes.clone(),
                    json_rpc_audit_req_id(payload.as_ref()),
                    started_wall_ms,
                    elapsed_ms(started_mono),
                    AuditOutcome::Err {
                        code: -32_803,
                        message: format!("gateway failed to publish: {error}"),
                    },
                    Some(payload.as_ref()),
                    enrich_audit_caller(
                        AuditEnvelopeFields {
                            trace_id: Some(trace_id.clone()),
                            rules_fired: Some(rules_fired),
                            rewrites,
                            stream_consumer,
                            zed_token_snapshot: tier1_zed_token.clone(),
                            ..Default::default()
                        },
                        &caller,
                    ),
                ),
            );
        }
        ForwardDisposition::Deadline => {
            tracing::Span::current().record(ATTR_ROUTING_OUTCOME, ROUTING_DEADLINE_EXCEEDED);
            warn!(
                ingress.subject = %ingress_subject,
                agent_subject = %agent_subject,
                method = %method_dots,
                routing_outcome = "deadline_exceeded",
                "gateway unary publish exceeded deadline before agent reply routing",
            );
            let body = ingress_gateway_deadline_exceeded_response_bytes(
                &headers_owned,
                payload.as_ref(),
                "gateway publish deadline exceeded for message/send",
            );
            reply_error(client, reply, HeaderMap::new(), body).await;
            spawn_gateway_audit_publish(
                audit_enabled,
                client.clone(),
                config.a2a_prefix.clone(),
                agent_id.clone(),
                AuditEnvelope::new(
                    &agent_id,
                    method_slashes.clone(),
                    json_rpc_audit_req_id(payload.as_ref()),
                    started_wall_ms,
                    elapsed_ms(started_mono),
                    AuditOutcome::Err {
                        code: -32_800,
                        message: "gateway publish deadline exceeded for message/send".into(),
                    },
                    Some(payload.as_ref()),
                    enrich_audit_caller(
                        AuditEnvelopeFields {
                            trace_id: Some(trace_id.clone()),
                            rules_fired: Some(rules_fired),
                            rewrites,
                            stream_consumer,
                            zed_token_snapshot: tier1_zed_token.clone(),
                            ..Default::default()
                        },
                        &caller,
                    ),
                ),
            );
        }
    }
}

async fn handle_ingress_routing_failure(
    client: &async_nats::Client,
    msg: &async_nats::Message,
    reply: async_nats::Subject,
    reason: String,
) {
    tracing::Span::current().record(ATTR_ROUTING_OUTCOME, ROUTING_INGRESS_ERROR);
    warn!(
        ingress.subject = %msg.subject.as_str(),
        ingress.reply_present = true,
        routing_outcome = "ingress_error",
        reason = %reason,
        reply = %reply,
        "gateway ingress subject routing failed",
    );
    let headers = msg.headers.clone().unwrap_or_default();
    let body = ingress_invalid_request_response_bytes(&headers, &msg.payload, reason);
    if let Err(error) = client.publish_with_headers(reply, HeaderMap::new(), body).await {
        warn!(error = %error, "gateway failed to publish ingress error reply");
    }
}

enum Tier1SpiceDbStepOutcome {
    Allow { zed_token: Option<String> },
    Denied,
}

#[allow(clippy::too_many_arguments)]
async fn run_tier1_spicedb<'a, E: ReadEnv>(
    client: &async_nats::Client,
    reply: async_nats::Subject,
    config: &'a Config,
    tier1: &dyn SpiceDbTier1Gate,
    tier1_owner: Option<&Arc<dyn OwnerTupleEmitter>>,
    agent_id: &'a a2a_nats::A2aAgentId,
    method: &A2aMethod,
    method_slashes: &'a str,
    _headers_owned: &HeaderMap,
    payload: &'a [u8],
    caller: &'a SubjectPrincipal,
    caller_spicedb: &CallerSpiceDbSubject,
    audit_enabled: bool,
    started_wall_ms: u64,
    started_mono: Instant,
    trace_id: &'a str,
    env: &E,
) -> Tier1SpiceDbStepOutcome {
    let publisher_account = env
        .var(ENV_GATEWAY_JWT_AUDIENCE)
        .unwrap_or_else(|_| config.a2a_prefix.as_str().to_owned());
    let principal = tier1_principal_from_caller(caller_spicedb);
    let session = tier1_session_from_caller(caller, publisher_account.as_str());
    let payload_bytes = Bytes::copy_from_slice(payload);
    let params = json_rpc_params(payload);
    let tuple = match derive_tuple(method, agent_id, session.account(), &params) {
        Ok(tuple) => tuple,
        Err(_) => {
            run_tier1_deny(
                client,
                reply,
                config,
                agent_id,
                method_slashes,
                &payload_bytes,
                trace_id,
                audit_enabled,
                started_wall_ms,
                started_mono,
                caller,
                "tier-1 resource tuple derivation failed",
            )
            .await;
            return Tier1SpiceDbStepOutcome::Denied;
        }
    };
    match tier1.authorize(&session, &principal, &tuple).await {
        Tier1AuthorizeOutcome::Allowed { zed_token } => {
            if matches!(method, A2aMethod::MessageSend)
                && let Some(emitter) = tier1_owner
                && let Some(owner) = owner_tuple_for_message_send(agent_id, &params, caller_spicedb)
                && let Err(error) = emitter.emit_owner(&owner).await
            {
                warn!(
                    error = %error,
                    "gateway tier-1 owner tuple write failed -- dispatch continues",
                );
            }
            Tier1SpiceDbStepOutcome::Allow { zed_token }
        }
        Tier1AuthorizeOutcome::Denied | Tier1AuthorizeOutcome::TransportError => {
            run_tier1_deny(
                client,
                reply,
                config,
                agent_id,
                method_slashes,
                &payload_bytes,
                trace_id,
                audit_enabled,
                started_wall_ms,
                started_mono,
                caller,
                "tier-1 SpiceDB denied ingress",
            )
            .await;
            Tier1SpiceDbStepOutcome::Denied
        }
        Tier1AuthorizeOutcome::DeriveFailed => {
            run_tier1_deny(
                client,
                reply,
                config,
                agent_id,
                method_slashes,
                &payload_bytes,
                trace_id,
                audit_enabled,
                started_wall_ms,
                started_mono,
                caller,
                "tier-1 resource tuple derivation failed",
            )
            .await;
            Tier1SpiceDbStepOutcome::Denied
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_tier1_deny<'a>(
    client: &async_nats::Client,
    reply: async_nats::Subject,
    config: &'a Config,
    agent_id: &'a a2a_nats::A2aAgentId,
    method_slashes: &'a str,
    payload: &'a Bytes,
    trace_id: &'a str,
    audit_enabled: bool,
    started_wall_ms: u64,
    started_mono: Instant,
    caller: &'a SubjectPrincipal,
    message: &'a str,
) {
    let body = ingress_gateway_policy_denied_response_bytes(&HeaderMap::new(), payload.as_ref(), message);
    let ctx = Tier1DenialCtx {
        a2a_prefix: &config.a2a_prefix,
        agent_id,
        method_slashes,
        payload,
        trace_id,
        audit_enabled,
        started_wall_ms,
        started_mono,
        caller,
    };
    deny_tier1(client, reply, body, ctx, message).await;
}

fn owner_tuple_for_message_send(
    agent_id: &a2a_nats::A2aAgentId,
    params: &serde_json::Value,
    caller: &CallerSpiceDbSubject,
) -> Option<Tier1OwnerTuple> {
    let task_id = params.get("id").and_then(serde_json::Value::as_str)?;
    Tier1OwnerTuple::for_task(agent_id, task_id, caller.object_type(), caller.object_id()).ok()
}

enum ForwardDisposition {
    Ok,
    Deadline,
    Publish(async_nats::client::PublishError),
}

async fn forward_to_agent<E: ReadEnv>(
    client: &async_nats::Client,
    env: &E,
    agent_subject: &str,
    method_dots: &str,
    reply: async_nats::Subject,
    headers: HeaderMap,
    payload: Bytes,
) -> ForwardDisposition {
    let agent_sub = async_nats::Subject::from(agent_subject);
    match unary_deadline_for_method(env, method_dots) {
        Some(deadline) => match tokio::time::timeout(
            deadline,
            client.publish_with_reply_and_headers(agent_sub, reply, headers, payload),
        )
        .await
        {
            Ok(Ok(())) => ForwardDisposition::Ok,
            Ok(Err(err)) => ForwardDisposition::Publish(err),
            Err(_elapsed) => ForwardDisposition::Deadline,
        },
        None => match client
            .publish_with_reply_and_headers(agent_sub, reply, headers, payload)
            .await
        {
            Ok(()) => ForwardDisposition::Ok,
            Err(err) => ForwardDisposition::Publish(err),
        },
    }
}

fn build_rules_fired<E: ReadEnv>(
    tier1: &dyn SpiceDbTier1Gate,
    policy: &GatewayPolicyStack,
    tier3_rewrites: &[crate::policy::tier3_redaction::RedactionRewrite],
    env: &E,
) -> Vec<String> {
    let mut rules_fired = Vec::with_capacity(3);
    rules_fired.push(if tier1.is_enabled() {
        "gateway.tier1.spicedb_allowed".into()
    } else {
        "gateway.tier1.layer_disabled".into()
    });
    rules_fired.push(match policy.substrate.as_ref() {
        Some(sub) if sub.tier2.is_active() => "gateway.tier2.evaluated_allow".into(),
        Some(_) => "gateway.tier2.no_op_evaluated_true".into(),
        None => "gateway.tier2.layer_disabled".into(),
    });
    rules_fired.push(if gateway_tier3_redaction_enabled(env) {
        if tier3_rewrites.is_empty() {
            "gateway.tier3.evaluated_allow".into()
        } else {
            "gateway.tier3.redacted".into()
        }
    } else {
        "gateway.tier3.layer_disabled".into()
    });
    rules_fired
}

fn elapsed_ms(started_mono: Instant) -> u64 {
    started_mono.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests;
