//! `{prefix}.gateway.>` ingress dispatch orchestrator.
//!
//! Runs each ingress request through the full policy stack in
//! order: caller-identity resolution -> Tier-1 SpiceDB ->
//! Tier-1 declarative -> Tier-2 CEL -> Tier-3 redaction. Each
//! denial path replies on the caller inbox with the matching
//! JSON-RPC error code and publishes an audit envelope; the
//! allow path forwards to `{prefix}.agent.{agent}.{method}` and
//! (when configured) spawns a streaming pump for stream-shaped
//! methods.
//!
//! Gated behind `cfg(not(coverage))` because it binds the concrete
//! `async_nats::Client` + JetStream pump; the per-tier classifier
//! and denial-shaping helpers live in sibling modules and have
//! their own unit tests.

#![cfg(all(feature = "spicedb", not(coverage)))]

use std::sync::Arc;
use std::time::Instant;

use a2a_auth_callout::SpiceDbSubject;
use a2a_nats::A2aMethod;
use a2a_nats::audit::envelope::{AuditEnvelope, AuditEnvelopeFields, AuditOutcome, Tier1Decision, Tier3Decision};
use a2a_nats::{
    gateway_ingress_agent_and_method_dots, ingress_error_response_wire,
    ingress_gateway_deadline_exceeded_response_bytes, ingress_gateway_declarative_denied_response_bytes,
    ingress_gateway_policy_denied_response_bytes, ingress_gateway_tier3_refused_response_bytes,
    ingress_invalid_request_response_bytes,
};
use async_nats::HeaderMap;
use bytes::Bytes;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, error, info, warn};
use trogon_identity_types::aauth::headers as aauth_headers;
use trogon_std::env::ReadEnv;
use uuid::Uuid;

use crate::aauth::{AAuthMode, GatewayAAuthIngress};
use crate::config::Config;
use crate::gw_ingress_stream::{CallerKey, GatewayStreamingIngressConfig, StreamingIngressGate};
use crate::jwt_caller_identity::{
    GatewayCallerIdentityPolicy, JwtHeaderCallerIdentitySource, gateway_audit_caller_attribution,
    resolve_gateway_caller_identity,
};
use a2a_nats::catalog::import_gate::SpiceDbPrincipal;

use crate::policy::spicedb_tier1::{
    OwnerTupleEmitter, SpiceDbTier1Gate, Tier1AuthorizeOutcome, Tier1OwnerTuple, derive_tuple,
    tier1_principal_from_caller, tier1_session_from_principal,
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
use crate::runtime::aauth_env::{GatewayCallerIdentity, aauth_deny_rule_fired, gateway_caller_identity_after_aauth};
use crate::runtime::audit_publish::spawn_gateway_audit_publish;
use crate::runtime::env::{
    gateway_audit_publish_enabled, json_rpc_audit_req_id, json_rpc_params, unary_deadline_for_method, unix_epoch_ms,
};
use crate::runtime::policy_stack::GatewayPolicyStack;
use crate::runtime::reply::reply_error;
use crate::runtime::streaming::maybe_spawn_streaming_ingress_pump;
use crate::runtime::tier1::{enrich_audit_caller, tier1_declarative_context_from_ingress};
use crate::runtime::tier1_denial::{Tier1DenialCtx, deny_tier1};

const ANONYMOUS_CALLER: &str = "_";
const MESSAGE_SEND_METHOD_DOTS: &str = "message.send";
const JWT_AUDIENCE_ENV: &str = "A2A_GATEWAY_JWT_AUDIENCE";

const SPAN_GATEWAY_INGRESS_DISPATCH: &str = "gateway.ingress.dispatch";
const ATTR_CALLER_ID: &str = "caller_id";
const ATTR_AGENT_SUBJECT: &str = "agent_subject";
const ATTR_ROUTING_OUTCOME: &str = "routing_outcome";

const ROUTING_IGNORED_NO_REPLY: &str = "ignored_no_reply";
const ROUTING_AAUTH_DENIED: &str = "aauth_denied";
const ROUTING_TIER1_DENIED: &str = "tier1_denied";
const ROUTING_POLICY_DENIED: &str = "policy_denied";
const ROUTING_TIER3_REFUSED: &str = "tier3_refused";
const ROUTING_TIER3_ENGINE_ERROR: &str = "tier3_engine_error";
const ROUTING_FORWARDED: &str = "forwarded";
const ROUTING_FORWARD_FAILED: &str = "forward_failed";
const ROUTING_DEADLINE_EXCEEDED: &str = "deadline_exceeded";
const ROUTING_INGRESS_ERROR: &str = "ingress_error";

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
    aauth: Option<&GatewayAAuthIngress>,
    policy: &GatewayPolicyStack,
    message_caller_identity: &JwtHeaderCallerIdentitySource,
    caller_identity_policy: GatewayCallerIdentityPolicy,
    streaming_ingress_enabled: bool,
    streaming_ingress_config: GatewayStreamingIngressConfig,
    streaming_ingress_gate: &StreamingIngressGate,
    shutdown: CancellationToken,
    env: &E,
    msg: async_nats::Message,
) {
    let ingress_subject = msg.subject.as_str().to_owned();
    let reply_present = msg.reply.is_some();
    let headers_for_identity = msg.headers.clone().unwrap_or_default();
    let caller_identity = resolve_gateway_caller_identity(
        message_caller_identity,
        &msg,
        &headers_for_identity,
        caller_identity_policy,
    );
    let (audit_caller_id, audit_caller_source) = gateway_audit_caller_attribution(caller_identity);

    let span = tracing::info_span!(
        SPAN_GATEWAY_INGRESS_DISPATCH,
        gateway_ingress.subject = %ingress_subject,
        ingress.reply_present = reply_present,
        caller_id = tracing::field::Empty,
        agent_subject = tracing::field::Empty,
        routing_outcome = tracing::field::Empty,
        aauth_agent_id = tracing::field::Empty,
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

        match gateway_ingress_agent_and_method_dots(ingress_subject.as_str(), &config.a2a_prefix) {
            Ok((agent_id, method_dots)) => {
                dispatch_routed(
                    client,
                    config,
                    tier1,
                    tier1_owner,
                    tier1_declarative,
                    aauth,
                    policy,
                    streaming_ingress_enabled,
                    streaming_ingress_config,
                    streaming_ingress_gate,
                    shutdown,
                    env,
                    &msg,
                    reply,
                    ingress_subject.as_str(),
                    agent_id,
                    method_dots,
                    audit_caller_id,
                    audit_caller_source,
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
    aauth: Option<&GatewayAAuthIngress>,
    policy: &GatewayPolicyStack,
    streaming_ingress_enabled: bool,
    streaming_ingress_config: GatewayStreamingIngressConfig,
    streaming_ingress_gate: &StreamingIngressGate,
    shutdown: CancellationToken,
    env: &E,
    msg: &async_nats::Message,
    reply: async_nats::Subject,
    ingress_subject: &str,
    agent_id: a2a_nats::A2aAgentId,
    method_dots: String,
    mut audit_caller_id: String,
    mut audit_caller_source: Option<String>,
) {
    let agent_subject = format!(
        "{}.agents.{}.{}",
        config.a2a_prefix.as_str(),
        agent_id.as_str(),
        method_dots
    );
    let mut headers_owned = msg.headers.clone().unwrap_or_default();
    tracing::Span::current().record(ATTR_AGENT_SUBJECT, tracing::field::display(&agent_subject));

    let mut payload: Bytes = msg.payload.clone();
    let started_mono = Instant::now();
    let started_wall_ms = unix_epoch_ms();
    let trace_id = Uuid::new_v4().to_string();
    let audit_enabled = gateway_audit_publish_enabled(env);
    let method_slashes = method_dots.replace('.', "/");
    let _unary_deadline_guard = unary_deadline_for_method(env, method_dots.as_str());

    let mut caller_slug = caller_slug_from_audit_id(audit_caller_id.as_str());
    let mut tier1_zed_token: Option<String> = None;
    // Carries the verified `aa-auth+jwt`'s `jti` past the AAuth block below so
    // it can be attached as the forwarded message's `AAuth-Access` header
    // (draft "AAuth-Access Response Header") once the allow path is reached.
    let mut aauth_access_jti: Option<String> = None;

    // AAuth verifies caller identity before any authorization tier runs --
    // authentication must precede authorization.
    if let Some(aauth) = aauth
        && aauth.mode != AAuthMode::Off
    {
        let header_pairs: Vec<(String, String)> = headers_owned
            .iter()
            .flat_map(|(name, values)| values.iter().map(move |value| (name.to_string(), value.to_string())))
            .collect();
        let auth_token = headers_owned
            .get(aauth_headers::NATS_AUTH_TOKEN)
            .map(std::string::ToString::to_string);
        match aauth
            .resolve_nats(
                ingress_subject,
                Some(reply.as_str()),
                payload.as_ref(),
                &header_pairs,
                auth_token.as_deref(),
                method_dots.as_str(),
            )
            .await
        {
            Ok(resolution) => {
                if let Some(agent_id) = resolution.agent_id.as_deref() {
                    tracing::Span::current().record("aauth_agent_id", agent_id);
                }
                // A verified `aa-auth+jwt` principal supersedes the
                // JWT-header caller identity for everything downstream
                // (Tier-1 authorization + audit attribution). Absent a
                // principal, the agent authenticated on its own behalf and
                // the existing caller identity is left untouched.
                let identity = gateway_caller_identity_after_aauth(
                    GatewayCallerIdentity {
                        audit_caller_id,
                        audit_caller_source,
                        caller_slug,
                    },
                    resolution.principal.as_deref(),
                );
                audit_caller_id = identity.audit_caller_id;
                audit_caller_source = identity.audit_caller_source;
                caller_slug = identity.caller_slug;
                aauth_access_jti = resolution.auth_jti;
            }
            Err(deny) => {
                tracing::Span::current().record(ATTR_ROUTING_OUTCOME, ROUTING_AAUTH_DENIED);
                warn!(
                    ingress.subject = %ingress_subject,
                    agent_subject = %agent_subject,
                    routing_outcome = ROUTING_AAUTH_DENIED,
                    reason = %deny.reason,
                    "gateway aauth verification rejected ingress envelope",
                );
                // Full wire encoding, not just the body: the JSON-RPC-over-
                // NATS binding discriminates errors via the Jsonrpc-Error-Code
                // and Jsonrpc-Id headers, so the deny must carry them for
                // clients to see -32118 at all.
                let Ok(encoded) = ingress_error_response_wire(
                    &headers_owned,
                    payload.as_ref(),
                    deny.code,
                    "aauth verification rejected envelope",
                    None,
                ) else {
                    return;
                };
                let mut reply_headers = encoded.headers;
                if let Some((name, value)) = deny.to_requirement_header() {
                    reply_headers.insert(name.as_str(), value.as_str());
                }
                reply_error(client, reply, reply_headers, encoded.body).await;
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
                            code: -32_118,
                            message: "aauth verification rejected envelope".into(),
                        },
                        Some(payload.as_ref()),
                        enrich_audit_caller(
                            AuditEnvelopeFields {
                                trace_id: Some(trace_id.clone()),
                                rules_fired: Some(vec![aauth_deny_rule_fired(&deny.reason).to_string()]),
                                ..Default::default()
                            },
                            audit_caller_id.as_str(),
                            &audit_caller_source,
                        ),
                    ),
                );
                return;
            }
        }
    }

    // AAuth-Access (draft "AAuth-Access Response Header"): once an
    // `aa-auth+jwt` has been verified, the resource echoes back an opaque
    // access identifier -- here, the auth token's `jti` -- so the forwarded
    // agent can correlate this delivery with the access grant that produced
    // it. Absent a verified auth token, no header is attached.
    if let Some(jti) = aauth_access_jti.as_deref() {
        headers_owned.insert(aauth_headers::ACCESS, jti);
    }

    if audit_caller_id != ANONYMOUS_CALLER {
        tracing::Span::current().record(ATTR_CALLER_ID, audit_caller_id.as_str());
    }

    if tier1.is_enabled() {
        match run_tier1_spicedb(
            client,
            reply.clone(),
            config,
            tier1,
            tier1_owner,
            &agent_id,
            method_dots.as_str(),
            method_slashes.as_str(),
            &headers_owned,
            payload.as_ref(),
            caller_slug.as_deref(),
            audit_caller_id.as_str(),
            &audit_caller_source,
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
        && let Some(declarative_ctx) = tier1_declarative_context_from_ingress(
            method_dots.as_str(),
            &agent_id,
            Some(audit_caller_id.as_str()),
            config.a2a_prefix.as_str(),
            ingress_subject,
        )
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
            let Ok(body) = ingress_gateway_declarative_denied_response_bytes(
                &headers_owned,
                payload.as_ref(),
                "tier-1 declarative policy rejected envelope",
            ) else {
                return;
            };
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
                        audit_caller_id.as_str(),
                        &audit_caller_source,
                    ),
                ),
            );
            return;
        }
    }

    if let Some(substrate) = policy.substrate.as_ref()
        && let Some(evaluator) = substrate.tier2.evaluator()
        && let Some(method) = A2aMethod::from_dotted_suffix(method_dots.as_str())
    {
        let caller_subject = SpiceDbSubject::new(audit_caller_id.as_str());
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
                let Ok(body) = ingress_gateway_policy_denied_response_bytes(
                    &headers_owned,
                    payload.as_ref(),
                    "tier-2 predicate rejected envelope",
                ) else {
                    return;
                };
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
                            audit_caller_id.as_str(),
                            &audit_caller_source,
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
        caller_slug.clone(),
        payload.as_ref(),
        policy.tier3_manifests.clone(),
    ) {
        Ok(ctx) => ctx,
        Err(_parse_err) => {
            // Unparseable payload -- fail closed rather than fall
            // through to a Null-payload manifest miss. Audit + reply
            // share the same -32801 shape as a policy denial.
            let Ok(body) = ingress_gateway_policy_denied_response_bytes(
                &headers_owned,
                payload.as_ref(),
                "tier-3 ingress payload not valid JSON",
            ) else {
                return;
            };
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
                    caller_id = caller_slug.as_deref().unwrap_or(""),
                    method = %method_slashes,
                    "gateway tier-3 redaction applied before forward",
                );
            }
            match tier3_ctx.into_payload_bytes() {
                Ok(bytes) => payload = bytes,
                Err(err) => {
                    error!(
                        caller_id = caller_slug.as_deref().unwrap_or(""),
                        method = %method_slashes,
                        error = %err,
                        "tier-3 rewritten payload serialization failed; denying ingress",
                    );
                    let Ok(body) = ingress_gateway_policy_denied_response_bytes(
                        &headers_owned,
                        payload.as_ref(),
                        "tier-3 redaction engine error",
                    ) else {
                        return;
                    };
                    reply_error(client, reply, HeaderMap::new(), body).await;
                    return;
                }
            }
        }
        Tier3RedactionDecision::Refuse { reason, rule } => {
            tracing::Span::current().record(ATTR_ROUTING_OUTCOME, ROUTING_TIER3_REFUSED);
            warn!(
                skill_id = %rule,
                caller_id = caller_slug.as_deref().unwrap_or(""),
                method = %method_slashes,
                reason = %reason.as_str(),
                routing_outcome = "tier3_refused",
                "gateway tier-3 skill refused part redaction",
            );
            let Ok(body) = ingress_gateway_tier3_refused_response_bytes(
                &headers_owned,
                payload.as_ref(),
                "tier-3 skill refused part redaction",
                rule.as_str(),
            ) else {
                return;
            };
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
                    AuditEnvelopeFields {
                        trace_id: Some(trace_id.clone()),
                        rules_fired: Some(vec![format!("gateway.tier3.refused.{}", rule.as_str())]),
                        rewrites: tier3_redaction_audit_rewrites(&tier3_rewrites),
                        tier3_decision: Some(Tier3Decision::Refuse),
                        ..Default::default()
                    },
                ),
            );
            return;
        }
        Tier3RedactionDecision::Error { rule, kind } => {
            tracing::Span::current().record(ATTR_ROUTING_OUTCOME, ROUTING_TIER3_ENGINE_ERROR);
            error!(
                skill_id = %rule,
                caller_id = caller_slug.as_deref().unwrap_or(""),
                method = %method_slashes,
                kind = %kind.as_str(),
                routing_outcome = "tier3_engine_error",
                "gateway tier-3 redaction engine failed closed",
            );
            let Ok(body) = ingress_gateway_policy_denied_response_bytes(
                &headers_owned,
                payload.as_ref(),
                "tier-3 redaction engine error",
            ) else {
                return;
            };
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
                    AuditEnvelopeFields {
                        trace_id: Some(trace_id.clone()),
                        rules_fired: Some(vec!["gateway.tier3.engine_error".into()]),
                        rewrites: tier3_redaction_audit_rewrites(&tier3_rewrites),
                        tier3_decision: Some(Tier3Decision::Error),
                        ..Default::default()
                    },
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

    let rules_fired = build_rules_fired(tier1, policy, &tier3_rewrites, env, aauth.map(|a| a.mode));
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
            if streaming_ingress_enabled && let Ok(caller_key) = CallerKey::new(audit_caller_id.as_str()) {
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
                        audit_caller_id.as_str(),
                        &audit_caller_source,
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
                        audit_caller_id.as_str(),
                        &audit_caller_source,
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
            let Ok(body) = ingress_gateway_deadline_exceeded_response_bytes(
                &headers_owned,
                payload.as_ref(),
                "gateway publish deadline exceeded for message/send",
            ) else {
                return;
            };
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
                        audit_caller_id.as_str(),
                        &audit_caller_source,
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
    let body = match ingress_invalid_request_response_bytes(&headers, &msg.payload, reason) {
        Ok(b) => b,
        Err(error) => {
            warn!(error = %error, "failed to serialize JSON-RPC ingress error response");
            return;
        }
    };
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
    method_dots: &str,
    method_slashes: &'a str,
    _headers_owned: &HeaderMap,
    payload: &'a [u8],
    caller_slug: Option<&str>,
    audit_caller_id: &'a str,
    audit_caller_source: &'a Option<String>,
    audit_enabled: bool,
    started_wall_ms: u64,
    started_mono: Instant,
    trace_id: &'a str,
    env: &E,
) -> Tier1SpiceDbStepOutcome {
    let publisher_account = env
        .var(JWT_AUDIENCE_ENV)
        .unwrap_or_else(|_| config.a2a_prefix.as_str().to_owned());
    let caller_slug_str = caller_slug.unwrap_or("");
    let principal = tier1_principal_from_caller(caller_slug_str, publisher_account.as_str());
    let payload_bytes = Bytes::copy_from_slice(payload);
    let Some(session) = tier1_session_from_principal(&principal, publisher_account.as_str()) else {
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
            audit_caller_id,
            audit_caller_source,
            "tier-1 principal lacks session identity",
        )
        .await;
        return Tier1SpiceDbStepOutcome::Denied;
    };
    let params = json_rpc_params(payload);
    let Some(method) = A2aMethod::from_dotted_suffix(method_dots) else {
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
            audit_caller_id,
            audit_caller_source,
            "tier-1 unknown method suffix",
        )
        .await;
        return Tier1SpiceDbStepOutcome::Denied;
    };
    let tuple = match derive_tuple(&method, agent_id, session.account(), &params) {
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
                audit_caller_id,
                audit_caller_source,
                "tier-1 resource tuple derivation failed",
            )
            .await;
            return Tier1SpiceDbStepOutcome::Denied;
        }
    };
    match tier1.authorize(&session, &principal, &tuple).await {
        Tier1AuthorizeOutcome::Allowed { zed_token } => {
            if method_dots == MESSAGE_SEND_METHOD_DOTS
                && let Some(emitter) = tier1_owner
                && let Some(owner) = owner_tuple_for_message_send(agent_id, &params, &principal)
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
                audit_caller_id,
                audit_caller_source,
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
                audit_caller_id,
                audit_caller_source,
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
    audit_caller_id: &'a str,
    audit_caller_source: &'a Option<String>,
    message: &'a str,
) {
    let Ok(body) = ingress_gateway_policy_denied_response_bytes(&HeaderMap::new(), payload.as_ref(), message) else {
        return;
    };
    let ctx = Tier1DenialCtx {
        a2a_prefix: &config.a2a_prefix,
        agent_id,
        method_slashes,
        payload,
        trace_id,
        audit_enabled,
        started_wall_ms,
        started_mono,
        audit_caller_id,
        audit_caller_source,
    };
    deny_tier1(client, reply, body, ctx, message).await;
}

fn owner_tuple_for_message_send(
    agent_id: &a2a_nats::A2aAgentId,
    params: &serde_json::Value,
    principal: &SpiceDbPrincipal,
) -> Option<Tier1OwnerTuple> {
    let task_id = params.get("id").and_then(serde_json::Value::as_str)?;
    let subject = principal.spicedb_subject()?;
    let subject_str = subject.as_str();
    let (subject_type, subject_id) = subject_str.split_once('/').unwrap_or(("user", subject_str));
    Tier1OwnerTuple::for_task(agent_id, task_id, subject_type, subject_id).ok()
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
    aauth_mode: Option<crate::aauth::AAuthMode>,
) -> Vec<String> {
    let mut rules_fired = Vec::with_capacity(4);
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
    rules_fired.push(match aauth_mode {
        Some(AAuthMode::Enforce) => "gateway.aauth.enforced_allow".into(),
        Some(AAuthMode::Shadow) => "gateway.aauth.shadow".into(),
        Some(AAuthMode::Off) | None => "gateway.aauth.layer_disabled".into(),
    });
    rules_fired
}

fn caller_slug_from_audit_id(audit_caller_id: &str) -> Option<String> {
    Some(
        audit_caller_id
            .split_once('/')
            .map(|(_, id)| id)
            .unwrap_or(audit_caller_id)
            .to_owned(),
    )
}

fn elapsed_ms(started_mono: Instant) -> u64 {
    started_mono.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}
