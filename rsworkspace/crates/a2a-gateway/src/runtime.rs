use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use a2a_nats::audit::emitter::{AuditEmitter, NatsAuditEmitter};
use a2a_nats::audit::envelope::{AuditEnvelope, AuditEnvelopeFields, AuditOutcome};
use a2a_nats::constants::DEFAULT_OPERATION_TIMEOUT;
use a2a_nats::push::PushDlqDedupGate;
use a2a_nats::{
    gateway_ingress_agent_and_method_dots, ingress_gateway_deadline_exceeded_response_bytes,
    ingress_gateway_policy_denied_response_bytes, ingress_gateway_tier3_refused_response_bytes,
    ingress_invalid_request_response_bytes, NatsConfig,
};
use a2a_redaction::wasm_bundle_path::WasmBundlePath;
use a2a_redaction::{Ed25519PublicKey, SkillId};
use async_nats::HeaderMap;
use bytes::Bytes;
use futures::stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, error, info, warn};
use uuid::Uuid;

use trogon_std::env::ReadEnv;

use crate::config::{Args, Config, ConfigError, config_from_args};
use crate::gw_pull_backpressure;
use a2a_auth_callout::signing_key_source::signing_key_source_from_process_env;

use crate::jwt_caller_identity::{
    gateway_audit_caller_attribution, gateway_caller_identity_policy, gateway_jwt_audience,
    resolve_gateway_caller_identity, JwtHeaderCallerIdentitySource,
};
use crate::policy::spicedb_tier1::{
    OwnerTupleEmitter, SpiceDbTier1Gate, Tier1AuthorizeOutcome, Tier1SpiceDbBuildError, Tier1SpiceDbConfig,
    a2a_method_from_dots, derive_tuple, owner_tuple_for_message_send, tier1_principal_from_caller,
    tier1_session_from_principal,
};
use crate::policy::tier1_declarative::{
    Tier1DeclarativeBuildError, Tier1DeclarativeConfig, Tier1DeclarativeContext, Tier1DeclarativeDecision,
    Tier1DeclarativeGate, tier1_declarative_audit_rule_fired,
};
use crate::policy::tier2::Tier2Decision;
use crate::policy::tier2_cel::{
    tier2_evaluation_context_from_ingress, RealTier2CelEvaluator, Tier2CompiledBundle,
};
use crate::policy::tier3_redaction::{
    gateway_tier3_redaction_enabled, load_tier3_manifests_from_bundle, merge_forward_audit_rewrites,
    NoopTier3RedactionGate, RealTier3RedactionGate, Tier3EvaluationContext, Tier3RedactionDecision,
    Tier3RedactionGate, tier3_redaction_audit_rewrites,
};
use crate::policy::wasmtime_substrate::WasmtimeSubstrate;

struct GatewayPolicyStack {
    substrate: Option<Arc<WasmtimeSubstrate>>,
    tier3_gate: Arc<dyn Tier3RedactionGate>,
    tier3_manifests: BTreeMap<SkillId, crate::policy::Tier3SkillManifest>,
}

#[derive(Debug)]
pub enum RuntimeError {
    Config(ConfigError),
    NatsConnect(trogon_nats::ConnectError),
    Subscribe(String),
    Tier1Config(Tier1SpiceDbBuildError),
    Tier1DeclarativeConfig(Tier1DeclarativeBuildError),
    SigningKeySource(a2a_auth_callout::AuthCalloutError),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => write!(f, "{error}"),
            Self::NatsConnect(error) => write!(f, "NATS connection failed: {error}"),
            Self::Subscribe(msg) => write!(f, "gateway subscribe failed: {msg}"),
            Self::Tier1Config(error) => write!(f, "{error}"),
            Self::Tier1DeclarativeConfig(error) => write!(f, "{error}"),
            Self::SigningKeySource(error) => write!(f, "gateway signing key source: {error}"),
        }
    }
}

impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::NatsConnect(error) => Some(error),
            Self::Subscribe(_) => None,
            Self::Tier1Config(_) => None,
            Self::Tier1DeclarativeConfig(_) => None,
            Self::SigningKeySource(error) => Some(error),
        }
    }
}

impl From<Tier1DeclarativeBuildError> for RuntimeError {
    fn from(error: Tier1DeclarativeBuildError) -> Self {
        Self::Tier1DeclarativeConfig(error)
    }
}

impl From<Tier1SpiceDbBuildError> for RuntimeError {
    fn from(error: Tier1SpiceDbBuildError) -> Self {
        Self::Tier1Config(error)
    }
}

impl From<ConfigError> for RuntimeError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

pub async fn run_with_args<E: trogon_std::env::ReadEnv>(args: Args, env: &E) -> Result<(), RuntimeError> {
    let (config, nats_config) = config_from_args(args, env)?;

    run_with_config(config, nats_config, env).await
}

pub async fn run_with_config<E: trogon_std::env::ReadEnv>(
    config: Config,
    nats_config: NatsConfig,
    env: &E,
) -> Result<(), RuntimeError> {
    let connect_timeout = a2a_nats::nats_connect_timeout(env);
    let client = trogon_nats::connect(&nats_config, connect_timeout)
        .await
        .map_err(RuntimeError::NatsConnect)?;

    let policy_stack = gateway_policy_stack_from_env(env);
    let caller_identity_policy = gateway_caller_identity_policy(env);
    let signing_key_source = signing_key_source_from_process_env().map_err(RuntimeError::SigningKeySource)?;
    let jwt_audience = gateway_jwt_audience(env, config.a2a_prefix.as_str());
    let message_caller_identity =
        JwtHeaderCallerIdentitySource::new(signing_key_source, jwt_audience);
    let tier1_layer = Tier1SpiceDbConfig::from_env(env).await?;
    let tier1_declarative_layer = Tier1DeclarativeConfig::from_env(env)?;

    let gateway_subject_string = config.gateway_subscribe_subject();
    let gateway_subject = async_nats::Subject::from(gateway_subject_string.as_str());

    let mut ingress = match &config.queue_group {
        Some(q) => client
            .queue_subscribe(gateway_subject, q.clone())
            .await
            .map_err(|e| RuntimeError::Subscribe(e.to_string()))?,
        None => client
            .subscribe(gateway_subject)
            .await
            .map_err(|e| RuntimeError::Subscribe(e.to_string()))?,
    };

    info!(
        prefix = %config.a2a_prefix,
        gateway_subject = %gateway_subject_string,
        queue_group = config.queue_group.as_deref().unwrap_or("(none — ephemeral subscriber)"),
        servers = ?config.nats_servers,
        "gateway subscribed on ingress wildcard; routing to mapped agent RPC subjects"
    );

    let shutdown = CancellationToken::new();
    let shutdown_for_task = shutdown.clone();

    tokio::spawn(async move {
        trogon_std::signal::shutdown_signal().await;
        shutdown_for_task.cancel();
    });

    let mirror_settings = crate::push_dlq_mirror::push_dlq_mirror_settings(env);
    if mirror_settings.enabled {
        let js = async_nats::jetstream::new(client.clone());
        let mirror_prefix = config.a2a_prefix.clone();
        let mirror_durable = mirror_settings.durable.clone();
        let mirror_shutdown = shutdown.clone();
        let mirror_dedup = Arc::new(PushDlqDedupGate::from_env(env));
        tokio::spawn(async move {
            crate::push_dlq_mirror::run_push_dlq_mirror(
                js,
                mirror_prefix,
                mirror_durable,
                mirror_shutdown,
                mirror_dedup,
            )
            .await;
        });
        info!(
            durable = %mirror_settings.durable.as_str(),
            "push DLQ mirror background task started"
        );
    }

    if gw_pull_backpressure::gateway_events_pull_enabled(env) {
        let pull_client = client.clone();
        let pull_prefix = config.a2a_prefix.clone();
        let pull_config = gw_pull_backpressure::GatewayEventsPullConfig::from_env(env);
        let pull_shutdown = shutdown.clone();
        tokio::spawn(async move {
            gw_pull_backpressure::run_gateway_events_pull(
                pull_client,
                pull_prefix,
                pull_config,
                pull_shutdown,
            )
            .await;
        });
        info!(
            prefix = %config.a2a_prefix,
            "gateway events pull consumer task spawned (A2A_GATEWAY_EVENTS_PULL=on)"
        );
    }

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                info!(prefix = %config.a2a_prefix, "gateway shutdown signal received");
                break;
            }
            incoming = ingress.next() => {
                match incoming {
                    Some(msg) => {
                        dispatch_gateway_ingress(
                            &client,
                            &config,
                            tier1_layer.gate.as_ref(),
                            tier1_layer.owner_emitter.as_ref(),
                            tier1_declarative_layer.gate.as_ref(),
                            &policy_stack,
                            &message_caller_identity,
                            caller_identity_policy,
                            env,
                            msg,
                        )
                        .await;
                    }
                    None => {
                        warn!("gateway ingress NATS subscription closed");
                        break;
                    }
                }
            }
        }
    }

    drop(ingress);

    info!(prefix = %config.a2a_prefix, "A2A gateway shutdown complete");
    Ok(())
}

#[allow(clippy::too_many_arguments, clippy::needless_update)]
async fn dispatch_gateway_ingress<E: ReadEnv>(
    client: &async_nats::Client,
    config: &Config,
    tier1: &dyn SpiceDbTier1Gate,
    tier1_owner: Option<&Arc<dyn OwnerTupleEmitter>>,
    tier1_declarative: &dyn Tier1DeclarativeGate,
    policy: &GatewayPolicyStack,
    message_caller_identity: &JwtHeaderCallerIdentitySource,
    caller_identity_policy: crate::jwt_caller_identity::GatewayCallerIdentityPolicy,
    env: &E,
    msg: async_nats::Message,
) {
    let ingress_subject = msg.subject.as_str();
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
        "gateway.ingress.dispatch",
        gateway_ingress.subject = %ingress_subject,
        ingress.reply_present = reply_present,
        caller_id = tracing::field::Empty,
        agent_subject = tracing::field::Empty,
        routing_outcome = tracing::field::Empty,
    );

    async {
        debug!(
            ingress.subject = %msg.subject,
            ingress.reply_present = reply_present,
            payload_len = msg.payload.len(),
            "gateway ingress envelope received",
        );

        let Some(reply) = msg.reply.clone() else {
            tracing::Span::current().record("routing_outcome", "ignored_no_reply");
            warn!(
                ingress.subject = %msg.subject,
                ingress.reply_present = false,
                routing_outcome = "ignored_no_reply",
                "gateway ingress without reply inbox; ignoring",
            );
            return;
        };

        match gateway_ingress_agent_and_method_dots(msg.subject.as_str(), &config.a2a_prefix) {
            Ok((agent_id, method_dots)) => {
                let agent_subject = format!(
                    "{}.agent.{}.{}",
                    config.a2a_prefix.as_str(),
                    agent_id.as_str(),
                    method_dots
                );
                let headers_owned = msg.headers.clone().unwrap_or_default();
                if audit_caller_id != "_" {
                    tracing::Span::current().record("caller_id", audit_caller_id.as_str());
                }
                tracing::Span::current().record("agent_subject", tracing::field::display(&agent_subject));
                let mut payload = msg.payload.clone();
                let started_mono = Instant::now();
                let started_wall_ms = unix_epoch_ms();
                let trace_id = Uuid::new_v4().to_string();
                let audit_enabled = gateway_audit_publish_enabled(env);
                let method_slashes = method_dots.replace('.', "/");
                let _unary_deadline_guard = unary_deadline_for_method(env, method_dots.as_str());

                let mut tier1_zed_token: Option<String> = None;
                if tier1.is_enabled() {
                    let publisher_account = std::env::var("A2A_GATEWAY_JWT_AUDIENCE")
                        .unwrap_or_else(|_| config.a2a_prefix.as_str().to_owned());
                    let caller_slug = audit_caller_id
                        .split_once('/')
                        .map(|(_, id)| id)
                        .unwrap_or(audit_caller_id.as_str());
                    let principal = tier1_principal_from_caller(caller_slug, publisher_account.as_str());
                    let Some(session) =
                        tier1_session_from_principal(&principal, publisher_account.as_str())
                    else {
                        tracing::Span::current().record("routing_outcome", "tier1_denied");
                        deny_tier1(
                            client,
                            reply,
                            tier1_denial_ctx(
                                config,
                                &agent_id,
                                &method_slashes,
                                &payload,
                                &trace_id,
                                audit_enabled,
                                started_wall_ms,
                                started_mono,
                                &audit_caller_id,
                                &audit_caller_source,
                            ),
                            "tier-1 principal lacks session identity",
                        )
                        .await;
                        return;
                    };

                    let params = json_rpc_params(payload.as_ref());
                    let Some(method) = a2a_method_from_dots(method_dots.as_str()) else {
                        tracing::Span::current().record("routing_outcome", "tier1_denied");
                        deny_tier1(
                            client,
                            reply,
                            tier1_denial_ctx(
                                config,
                                &agent_id,
                                &method_slashes,
                                &payload,
                                &trace_id,
                                audit_enabled,
                                started_wall_ms,
                                started_mono,
                                &audit_caller_id,
                                &audit_caller_source,
                            ),
                            "tier-1 unknown method suffix",
                        )
                        .await;
                        return;
                    };

                    let tuple = match derive_tuple(&method, &agent_id, session.account(), &params) {
                        Ok(tuple) => tuple,
                        Err(_derive_err) => {
                            tracing::Span::current().record("routing_outcome", "tier1_denied");
                            deny_tier1(
                                client,
                                reply,
                                tier1_denial_ctx(
                                    config,
                                    &agent_id,
                                    &method_slashes,
                                    &payload,
                                    &trace_id,
                                    audit_enabled,
                                    started_wall_ms,
                                    started_mono,
                                    &audit_caller_id,
                                    &audit_caller_source,
                                ),
                                "tier-1 resource tuple derivation failed",
                            )
                            .await;
                            return;
                        }
                    };

                    match tier1.authorize(&session, &principal, &tuple).await {
                        Tier1AuthorizeOutcome::Allowed { zed_token } => {
                            tier1_zed_token = zed_token;
                            if method_dots == "message.send"
                                && let Some(owner_emitter) = tier1_owner
                                && let Some(owner) =
                                    owner_tuple_for_message_send(&agent_id, &params, &principal)
                                && let Err(error) = owner_emitter.emit_owner(&owner).await
                            {
                                warn!(
                                    ingress.subject = %msg.subject,
                                    agent_subject = %agent_subject,
                                    error = %error,
                                    "gateway tier-1 owner tuple write failed — dispatch continues",
                                );
                            }
                        }
                        Tier1AuthorizeOutcome::Denied | Tier1AuthorizeOutcome::TransportError => {
                            tracing::Span::current().record("routing_outcome", "tier1_denied");
                            deny_tier1(
                                client,
                                reply,
                                tier1_denial_ctx(
                                    config,
                                    &agent_id,
                                    &method_slashes,
                                    &payload,
                                    &trace_id,
                                    audit_enabled,
                                    started_wall_ms,
                                    started_mono,
                                    &audit_caller_id,
                                    &audit_caller_source,
                                ),
                                "tier-1 SpiceDB denied ingress",
                            )
                            .await;
                            return;
                        }
                        Tier1AuthorizeOutcome::DeriveFailed => {
                            tracing::Span::current().record("routing_outcome", "tier1_denied");
                            deny_tier1(
                                client,
                                reply,
                                tier1_denial_ctx(
                                    config,
                                    &agent_id,
                                    &method_slashes,
                                    &payload,
                                    &trace_id,
                                    audit_enabled,
                                    started_wall_ms,
                                    started_mono,
                                    &audit_caller_id,
                                    &audit_caller_source,
                                ),
                                "tier-1 resource tuple derivation failed",
                            )
                            .await;
                            return;
                        }
                    }
                }

                let mut tier1_declarative_audit: Option<String> = None;
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
                    tier1_declarative_audit = Some(tier1_declarative_audit_rule_fired(&decision));
                    if let Tier1DeclarativeDecision::Deny { rule } = decision {
                        let rule_fired = format!("gateway.tier1.declarative.denied.{}", rule.as_str());
                        tracing::Span::current().record("routing_outcome", "policy_denied");
                        warn!(
                            ingress.subject = %msg.subject,
                            agent_subject = %agent_subject,
                            rule = %rule,
                            routing_outcome = "policy_denied",
                            "gateway tier-1 declarative policy rejected ingress envelope",
                        );
                        let Ok(body) = ingress_gateway_policy_denied_response_bytes(
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
                                started_mono.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                                AuditOutcome::Err {
                                    code: -32_801,
                                    message: "tier-1 declarative policy rejected envelope".into(),
                                },
                                Some(payload.as_ref()),
                                AuditEnvelopeFields {
                                    trace_id: Some(trace_id.clone()),
                                    rules_fired: Some(vec![rule_fired]),
                                    zed_token_snapshot: tier1_zed_token.clone(),
                                    ..Default::default()
                                },
                            ),
                        );
                        return;
                    }
                }

                if let Some(sub) = policy.substrate.as_ref() {
                    let eval_ctx = tier2_evaluation_context_from_ingress(
                        &method_slashes,
                        &agent_id,
                        Some(audit_caller_id.as_str()),
                        &headers_owned,
                        payload.as_ref(),
                    );
                    match sub.tier2.evaluate(&eval_ctx) {
                        Tier2Decision::Allow => {}
                        Tier2Decision::Deny { rule } => {
                            let rule_fired = format!("gateway.tier2.{}", rule.as_str());
                            tracing::Span::current().record("routing_outcome", "policy_denied");
                            warn!(
                                ingress.subject = %msg.subject,
                                agent_subject = %agent_subject,
                                rule = %rule,
                                routing_outcome = "policy_denied",
                                "gateway tier-2 predicate rejected ingress envelope",
                            );
                            let Ok(body) = ingress_gateway_policy_denied_response_bytes(
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
                                    started_mono.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                                    AuditOutcome::Err {
                                        code: -32_801,
                                        message: "tier-2 predicate rejected envelope".into(),
                                    },
                                    Some(payload.as_ref()),
                                    enrich_audit_caller(
                                        AuditEnvelopeFields {
                                            trace_id: Some(trace_id.clone()),
                                            rules_fired: Some(vec![rule_fired]),
                                            ..Default::default()
                                        },
                                        &audit_caller_id,
                                        &audit_caller_source,
                                    ),
                                ),
                            );
                            return;
                        }
                    }
                }

                let mut tier3_rewrites = Vec::new();
                let tier3_caller = (audit_caller_id != "_").then(|| audit_caller_id.clone());
                let mut tier3_ctx = Tier3EvaluationContext::from_json_rpc_payload(
                    method_slashes.clone(),
                    tier3_caller,
                    payload.as_ref(),
                    policy.tier3_manifests.clone(),
                );
                match policy.tier3_gate.redact(&mut tier3_ctx) {
                    Tier3RedactionDecision::Allow { rewrites } => {
                        tier3_rewrites = rewrites;
                        if !tier3_rewrites.is_empty() {
                            info!(
                                count = tier3_rewrites.len(),
                                caller_id = audit_caller_id.as_str(),
                                method = %method_slashes,
                                "gateway tier-3 redaction applied before forward",
                            );
                        }
                        match tier3_ctx.into_payload_bytes() {
                            Ok(bytes) => payload = bytes,
                            Err(err) => {
                                error!(
                                    caller_id = audit_caller_id.as_str(),
                                    method = %method_slashes,
                                    error = %err,
                                    "tier-3 rewritten payload serialization failed; denying ingress",
                                );
                                let Ok(body) = ingress_gateway_policy_denied_response_bytes(
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
                                        started_mono
                                            .elapsed()
                                            .as_millis()
                                            .min(u128::from(u64::MAX))
                                            as u64,
                                        AuditOutcome::Err {
                                            code: -32_801,
                                            message: "tier-3 redaction engine error".into(),
                                        },
                                        Some(payload.as_ref()),
                                        AuditEnvelopeFields {
                                            trace_id: Some(trace_id.clone()),
                                            rules_fired: Some(vec!["gateway.tier3.engine_error".into()]),
                                            rewrites: tier3_redaction_audit_rewrites(&tier3_rewrites),
                                            ..Default::default()
                                        },
                                    ),
                                );
                                return;
                            }
                        }
                    }
                    Tier3RedactionDecision::Refuse { reason, rule } => {
                        tracing::Span::current().record("routing_outcome", "tier3_refused");
                        warn!(
                            skill_id = %rule,
                            caller_id = audit_caller_id.as_str(),
                            method = %method_slashes,
                            reason = %reason.as_str(),
                            routing_outcome = "tier3_refused",
                            "gateway tier-3 skill refused part redaction",
                        );
                        let Ok(body) = ingress_gateway_tier3_refused_response_bytes(
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
                                started_mono.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                                AuditOutcome::Err {
                                    code: -32_802,
                                    message: format!("tier-3 skill refused: {}", reason.as_str()),
                                },
                                Some(payload.as_ref()),
                                AuditEnvelopeFields {
                                    trace_id: Some(trace_id.clone()),
                                    rules_fired: Some(vec![format!("gateway.tier3.refused.{}", rule.as_str())]),
                                    rewrites: tier3_redaction_audit_rewrites(&tier3_rewrites),
                                    refusal_skill: Some(rule.as_str().to_owned()),
                                    ..Default::default()
                                },
                            ),
                        );
                        return;
                    }
                    Tier3RedactionDecision::Error { rule, kind } => {
                        tracing::Span::current().record("routing_outcome", "tier3_engine_error");
                        error!(
                            skill_id = %rule,
                            caller_id = audit_caller_id.as_str(),
                            method = %method_slashes,
                            kind = %kind.as_str(),
                            routing_outcome = "tier3_engine_error",
                            "gateway tier-3 redaction engine failed closed",
                        );
                        let Ok(body) = ingress_gateway_policy_denied_response_bytes(
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
                                started_mono.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                                AuditOutcome::Err {
                                    code: -32_801,
                                    message: "tier-3 redaction engine error".into(),
                                },
                                Some(payload.as_ref()),
                                AuditEnvelopeFields {
                                    trace_id: Some(trace_id.clone()),
                                    rules_fired: Some(vec!["gateway.tier3.engine_error".into()]),
                                    rewrites: tier3_redaction_audit_rewrites(&tier3_rewrites),
                                    ..Default::default()
                                },
                            ),
                        );
                        return;
                    }
                }

                debug!(
                    ingress.subject = %msg.subject,
                    agent_subject = %agent_subject,
                    ingress.reply_present = true,
                    reply = %reply,
                    "gateway forwarding to agent subject",
                );

                enum ForwardDisposition {
                    Ok,
                    Deadline,
                    Publish(async_nats::client::PublishError),
                }

                let disposition = match unary_deadline_for_method(env, method_dots.as_str()) {
                    Some(deadline) => match tokio::time::timeout(
                        deadline,
                        client.clone().publish_with_reply_and_headers(
                            async_nats::Subject::from(agent_subject.clone().as_str()),
                            reply.clone(),
                            headers_owned.clone(),
                            payload.clone(),
                        ),
                    )
                    .await
                    {
                        Ok(Ok(())) => ForwardDisposition::Ok,
                        Ok(Err(err)) => ForwardDisposition::Publish(err),
                        Err(_elapsed) => ForwardDisposition::Deadline,
                    },
                    None => match client
                        .publish_with_reply_and_headers(
                            async_nats::Subject::from(agent_subject.clone().as_str()),
                            reply.clone(),
                            headers_owned,
                            payload.clone(),
                        )
                        .await
                    {
                        Ok(()) => ForwardDisposition::Ok,
                        Err(err) => ForwardDisposition::Publish(err),
                    },
                };

                let mut rules_fired: Vec<String> = Vec::new();
                if tier1.is_enabled() {
                    rules_fired.push("gateway.tier1.spicedb_allowed".into());
                } else {
                    rules_fired.push("gateway.tier1.layer_disabled".into());
                }
                if tier1_declarative.is_enabled() {
                    rules_fired.push(
                        tier1_declarative_audit
                            .unwrap_or_else(|| "gateway.tier1.declarative.no_match_default_allow".into()),
                    );
                } else {
                    rules_fired.push("gateway.tier1.declarative.layer_disabled".into());
                }
                if let Some(sub) = policy.substrate.as_ref() {
                    if sub.tier2_cel_active {
                        rules_fired.push("gateway.tier2.evaluated_allow".into());
                    } else {
                        rules_fired.push("gateway.tier2.no_op_evaluated_true".into());
                    }
                } else {
                    rules_fired.push("gateway.tier2.layer_disabled".into());
                }
                if gateway_tier3_redaction_enabled(env) {
                    if tier3_rewrites.is_empty() {
                        rules_fired.push("gateway.tier3.evaluated_allow".into());
                    } else {
                        rules_fired.push("gateway.tier3.redacted".into());
                    }
                } else {
                    rules_fired.push("gateway.tier3.layer_disabled".into());
                }
                let (rewrites, stream_consumer) = merge_forward_audit_rewrites(
                    &tier3_rewrites,
                    ingress_subject,
                    &agent_subject,
                    &agent_id,
                    method_dots.as_str(),
                );

                match disposition {
                    ForwardDisposition::Ok => {
                        tracing::Span::current().record("routing_outcome", "forwarded");
                        spawn_gateway_audit_publish(
                            audit_enabled,
                            client.clone(),
                            config.a2a_prefix.clone(),
                            agent_id.clone(),
                            AuditEnvelope::new(
                                &agent_id,
                                method_slashes,
                                json_rpc_audit_req_id(payload.as_ref()),
                                started_wall_ms,
                                started_mono.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                                AuditOutcome::Ok,
                                Some(payload.as_ref()),
                                enrich_audit_caller(
                                    AuditEnvelopeFields {
                                        trace_id: Some(trace_id),
                                        rules_fired: Some(rules_fired),
                                        rewrites,
                                        stream_consumer,
                                        zed_token_snapshot: tier1_zed_token.clone(),
                                        ..Default::default()
                                    },
                                    &audit_caller_id,
                                    &audit_caller_source,
                                ),
                            ),
                        );
                    }
                    ForwardDisposition::Publish(error) => {
                        tracing::Span::current().record("routing_outcome", "forward_failed");
                        warn!(
                            ingress.subject = %msg.subject,
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
                                method_slashes,
                                json_rpc_audit_req_id(payload.as_ref()),
                                started_wall_ms,
                                started_mono.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                                AuditOutcome::Err {
                                    code: -32_803,
                                    message: format!("gateway failed to publish: {error}"),
                                },
                                Some(payload.as_ref()),
                                enrich_audit_caller(
                                    AuditEnvelopeFields {
                                        trace_id: Some(trace_id),
                                        rules_fired: Some(rules_fired.clone()),
                                        rewrites: rewrites.clone(),
                                        stream_consumer: stream_consumer.clone(),
                                        zed_token_snapshot: tier1_zed_token.clone(),
                                        ..Default::default()
                                    },
                                    &audit_caller_id,
                                    &audit_caller_source,
                                ),
                            ),
                        );
                    }
                    ForwardDisposition::Deadline => {
                        tracing::Span::current().record("routing_outcome", "deadline_exceeded");
                        warn!(
                            ingress.subject = %msg.subject,
                            agent_subject = %agent_subject,
                            method = %method_dots,
                            routing_outcome = "deadline_exceeded",
                            "gateway unary publish exceeded deadline before agent reply routing",
                        );
                        let Ok(body) = ingress_gateway_deadline_exceeded_response_bytes(
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
                                method_slashes,
                                json_rpc_audit_req_id(payload.as_ref()),
                                started_wall_ms,
                                started_mono.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                                AuditOutcome::Err {
                                    code: -32_800,
                                    message: "gateway publish deadline exceeded for message/send".into(),
                                },
                                Some(payload.as_ref()),
                                enrich_audit_caller(
                                    AuditEnvelopeFields {
                                        trace_id: Some(trace_id),
                                        rules_fired: Some(rules_fired),
                                        rewrites,
                                        stream_consumer,
                                        zed_token_snapshot: tier1_zed_token.clone(),
                                        ..Default::default()
                                    },
                                    &audit_caller_id,
                                    &audit_caller_source,
                                ),
                            ),
                        );
                    }
                }
            }
            Err(reason) => {
                tracing::Span::current().record("routing_outcome", "ingress_error");
                warn!(
                    ingress.subject = %msg.subject,
                    ingress.reply_present = true,
                    routing_outcome = "ingress_error",
                    reason = %reason,
                    reply = %reply,
                    "gateway ingress subject routing failed",
                );
                let body = match ingress_invalid_request_response_bytes(&msg.payload, reason.to_string()) {
                    Ok(b) => b,
                    Err(error) => {
                        warn!(
                            ingress.subject = %msg.subject,
                            ingress.reply_present = true,
                            routing_outcome = "ingress_error",
                            error = %error,
                            "failed to serialize JSON-RPC ingress error response",
                        );
                        return;
                    }
                };
                let headers = HeaderMap::new();
                if let Err(error) = client.publish_with_headers(reply.clone(), headers, body).await {
                    warn!(
                        ingress.subject = %msg.subject,
                        ingress.reply_present = true,
                        routing_outcome = "ingress_error",
                        reply = %reply,
                        error = %error,
                        "gateway failed to publish ingress error reply",
                    );
                }
            }
        }
    }
    .instrument(span)
    .await
}

struct Tier1DenialCtx<'a> {
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
}

#[allow(clippy::too_many_arguments)]
fn tier1_denial_ctx<'a>(
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
) -> Tier1DenialCtx<'a> {
    Tier1DenialCtx {
        config,
        agent_id,
        method_slashes,
        payload,
        trace_id,
        audit_enabled,
        started_wall_ms,
        started_mono,
        audit_caller_id,
        audit_caller_source,
    }
}

fn enrich_audit_caller(
    mut fields: AuditEnvelopeFields,
    caller_id: &str,
    caller_source: &Option<String>,
) -> AuditEnvelopeFields {
    fields.caller_id = Some(caller_id.to_owned());
    fields.caller_source = caller_source.clone();
    fields
}

async fn deny_tier1(
    client: &async_nats::Client,
    reply: async_nats::Subject,
    ctx: Tier1DenialCtx<'_>,
    message: &str,
) {
    warn!(
        agent_id = %ctx.agent_id,
        method = %ctx.method_slashes,
        routing_outcome = "tier1_denied",
        "gateway tier-1 SpiceDB denied ingress",
    );
    let Ok(body) = ingress_gateway_policy_denied_response_bytes(ctx.payload.as_ref(), message) else {
        return;
    };
    reply_error(client, reply, HeaderMap::new(), body).await;
    spawn_gateway_audit_publish(
        ctx.audit_enabled,
        client.clone(),
        ctx.config.a2a_prefix.clone(),
        ctx.agent_id.clone(),
        AuditEnvelope::new(
            ctx.agent_id,
            ctx.method_slashes,
            json_rpc_audit_req_id(ctx.payload.as_ref()),
            ctx.started_wall_ms,
            ctx.started_mono
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
            AuditOutcome::Err {
                code: -32_801,
                message: message.into(),
            },
            Some(ctx.payload.as_ref()),
            enrich_audit_caller(
                AuditEnvelopeFields {
                    trace_id: Some(ctx.trace_id.to_owned()),
                    rules_fired: Some(vec!["gateway.tier1.spicedb_denied".into()]),
                    ..Default::default()
                },
                ctx.audit_caller_id,
                ctx.audit_caller_source,
            ),
        ),
    );
}

fn json_rpc_params(payload: &[u8]) -> serde_json::Value {
    serde_json::from_slice::<serde_json::Value>(payload)
        .ok()
        .and_then(|value| value.get("params").cloned())
        .unwrap_or(serde_json::Value::Object(Default::default()))
}

fn gateway_policy_stack_from_env<E: ReadEnv>(env: &E) -> GatewayPolicyStack {
    let tier3_enabled = gateway_tier3_redaction_enabled(env);
    let noop_stack = || GatewayPolicyStack {
        substrate: None,
        tier3_gate: Arc::new(NoopTier3RedactionGate),
        tier3_manifests: BTreeMap::new(),
    };

    let Some(raw) = env.var("A2A_GATEWAY_POLICY_BUNDLE_DIR").ok() else {
        if tier3_enabled {
            warn!("A2A_GATEWAY_TIER3_REDACTION_ENABLED=on but A2A_GATEWAY_POLICY_BUNDLE_DIR unset; tier-3 noop");
        }
        return noop_stack();
    };
    let dir = raw.trim();
    if dir.is_empty() {
        if tier3_enabled {
            warn!("A2A_GATEWAY_TIER3_REDACTION_ENABLED=on but A2A_GATEWAY_POLICY_BUNDLE_DIR empty; tier-3 noop");
        }
        return noop_stack();
    }

    let bundle_path = WasmBundlePath::new(dir);
    let tier3_signing_pubkey = gateway_tier3_signing_pubkey(env);
    let tier2_cel_active = gateway_tier2_cel_enabled(env);
    let tier2: Box<dyn crate::policy::Tier2CelEvaluator> = if tier2_cel_active {
        let tier2_dir = bundle_path.as_path().join("tier2");
        match Tier2CompiledBundle::load_from_dir(&tier2_dir) {
            Ok(bundle) => Box::new(RealTier2CelEvaluator::new(bundle)),
            Err(err) => {
                warn!(
                    error = %err,
                    tier2_dir = %tier2_dir.display(),
                    "A2A_GATEWAY_TIER2_CEL_ENABLED=on but tier2 bundle load failed — denying all ingress",
                );
                Box::new(crate::policy::tier2::DenyAllTier2Evaluator)
            }
        }
    } else {
        Box::new(crate::policy::NoopTier2Evaluator)
    };

    match WasmtimeSubstrate::try_new_with_tier2(
        bundle_path.clone(),
        tier2,
        tier2_cel_active,
        tier3_signing_pubkey,
    ) {
        Err(err) => {
            warn!(
                error = %err,
                bundle_dir = dir,
                "A2A_GATEWAY_POLICY_BUNDLE_DIR invalid — Wasmtime substrate disabled",
            );
            if tier3_enabled {
                warn!("A2A_GATEWAY_TIER3_REDACTION_ENABLED=on but substrate failed to load; tier-3 noop");
            }
            noop_stack()
        }
        Ok(layer) => {
            let substrate = Arc::new(layer);
            let mut loaded_skills = Vec::new();
            if let Ok(slugs) = env.var("A2A_GATEWAY_POLICY_SKILLS") {
                for slug in slugs.split(',').map(str::trim).filter(|slug| !slug.is_empty()) {
                    let skill_id = SkillId::new(slug);
                    match substrate.preload_redaction_skill(skill_id.clone()) {
                        Err(err) => {
                            error!(skill=%slug, error=%err, "gateway tier-3 policy bundle preload failed");
                        }
                        Ok(()) => {
                            loaded_skills.push(skill_id);
                        }
                    }
                }
            }

            let tier3_manifests = load_tier3_manifests_from_bundle(&bundle_path, &loaded_skills);
            let tier3_gate: Arc<dyn Tier3RedactionGate> = if tier3_enabled {
                Arc::new(RealTier3RedactionGate::from_substrate(substrate.clone()))
            } else {
                Arc::new(NoopTier3RedactionGate)
            };

            GatewayPolicyStack {
                substrate: Some(substrate),
                tier3_gate,
                tier3_manifests,
            }
        }
    }
}

fn gateway_tier2_cel_enabled<E: ReadEnv>(env: &E) -> bool {
    let Ok(flag) = env.var("A2A_GATEWAY_TIER2_CEL_ENABLED") else {
        return false;
    };
    matches!(
        flag.to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn gateway_tier3_signing_pubkey<E: ReadEnv>(env: &E) -> Option<Ed25519PublicKey> {
    let Ok(raw) = env.var("A2A_GATEWAY_TIER3_SIGNING_PUBKEY") else {
        return None;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    match Ed25519PublicKey::from_hex(trimmed) {
        Ok(pubkey) => Some(pubkey),
        Err(err) => {
            warn!(error=%err, "A2A_GATEWAY_TIER3_SIGNING_PUBKEY invalid; tier-3 bundle signing disabled");
            None
        }
    }
}

fn gateway_audit_publish_enabled<E: ReadEnv>(env: &E) -> bool {
    let Ok(flag) = env.var("A2A_GATEWAY_AUDIT_PUBLISH") else {
        return false;
    };
    matches!(
        flag.to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn unary_deadline_for_method<E: ReadEnv>(env: &E, method_dots: &str) -> Option<Duration> {
    if method_dots != "message.send" {
        return None;
    }

    let secs: u64 = env
        .var("A2A_GATEWAY_UNARY_DEADLINE_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_OPERATION_TIMEOUT.as_secs())
        .max(1);

    Some(Duration::from_secs(secs))
}

fn tier1_declarative_context_from_ingress(
    method_dots: &str,
    agent_id: &a2a_nats::A2aAgentId,
    caller_slug: Option<&str>,
    account: &str,
    nats_subject: &str,
) -> Option<Tier1DeclarativeContext> {
    let method = a2a_method_from_dots(method_dots)?;
    let raw_slug = caller_slug.unwrap_or("_");
    let slug = raw_slug
        .split_once('/')
        .map(|(_, id)| id)
        .unwrap_or(raw_slug);
    let principal = tier1_principal_from_caller(slug, account);
    let caller_subject = principal.spicedb_subject();
    Some(Tier1DeclarativeContext::new(
        method,
        agent_id.clone(),
        caller_subject,
        nats_subject,
    ))
}


fn unix_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn json_rpc_audit_req_id(payload: &[u8]) -> Option<String> {
    a2a_nats::extract_request_id(payload).map(|id| id.to_string())
}

async fn reply_error(client: &async_nats::Client, reply: async_nats::Subject, hdrs: HeaderMap, body: Bytes) {
    if let Err(error) = client.publish_with_headers(reply, hdrs, body).await {
        warn!(error=%error, routing_outcome = "error_reply_publish_failed");
    }
}

fn spawn_gateway_audit_publish(
    enabled: bool,
    client: async_nats::Client,
    prefix: a2a_nats::A2aPrefix,
    agent_id: a2a_nats::A2aAgentId,
    envelope: AuditEnvelope,
) {
    if !enabled {
        return;
    }

    tokio::spawn(async move {
        let emitter = NatsAuditEmitter::new(client);
        emitter.publish(&prefix, &agent_id, envelope).await;
    });
}

#[cfg(test)]
mod gateway_dispatch_tests {
    use a2a_nats::agent_id::A2aAgentId;
    use a2a_nats::audit::envelope::{AuditEnvelope, AuditEnvelopeFields, AuditOutcome, gateway_forward_audit_extras};
    use a2a_nats::ingress_gateway_policy_denied_response_bytes;
    use a2a_nats::resolve_gateway_ingress_subject;
    use async_nats::HeaderMap;
    use trogon_std::env::InMemoryEnv;

    use super::{config_from_args, tier1_declarative_context_from_ingress, Args, Config};
    use crate::policy::tier1_declarative::{
        RealTier1DeclarativeGate, Tier1DeclarativeBundle, Tier1DeclarativeDecision, Tier1DeclarativeGate,
        tier1_declarative_audit_rule_fired,
    };
    use crate::policy::tier2::{Tier2CelEvaluator, Tier2Decision};
    use crate::policy::tier2_cel::tier2_evaluation_context_from_ingress;
    use crate::policy::RuleName;
    use crate::policy::tier2_cel::{RealTier2CelEvaluator, Tier2CompiledBundle};

    fn test_config(prefix: &str) -> Config {
        let env = InMemoryEnv::new();
        let args = Args {
            nats_url: "localhost:4222".into(),
            prefix: prefix.into(),
            queue_group: None,
        };
        config_from_args(args, &env).unwrap().0
    }

    fn test_agent() -> A2aAgentId {
        A2aAgentId::new("planner").unwrap()
    }

    #[allow(clippy::needless_update)]
    fn forward_audit_fields(
        ingress_subject: &str,
        agent_subject: &str,
        agent_id: &A2aAgentId,
        method_dots: &str,
    ) -> AuditEnvelopeFields {
        let (rewrites, stream_consumer) =
            gateway_forward_audit_extras(ingress_subject, agent_subject, agent_id, method_dots);
        AuditEnvelopeFields {
            trace_id: Some("trace-test".into()),
            rules_fired: Some(vec!["gateway.tier2.layer_disabled".into()]),
            rewrites,
            stream_consumer,
            zed_token_snapshot: None,
            ..Default::default()
        }
    }

    fn denial_audit_fields(rule: &str) -> AuditEnvelopeFields {
        AuditEnvelopeFields {
            trace_id: Some("trace-test".into()),
            rules_fired: Some(vec![format!("gateway.tier2.{rule}")]),
            ..Default::default()
        }
    }

    #[test]
    fn dispatch_builds_publish_args_from_valid_ingress_subject() {
        let cfg = test_config("a2a");
        assert_eq!(
            resolve_gateway_ingress_subject("a2a.gateway.planner.message.send", &cfg.a2a_prefix).unwrap(),
            "a2a.agent.planner.message.send"
        );
    }

    #[test]
    fn forward_ok_audit_includes_rewrite_and_omits_stream_consumer_for_unary() {
        let agent = test_agent();
        let ingress = "a2a.gateway.planner.message.send";
        let agent_subject = "a2a.agent.planner.message.send";
        let extras = forward_audit_fields(ingress, agent_subject, &agent, "message.send");
        let envelope = AuditEnvelope::new(
            &agent,
            "message/send",
            None,
            0,
            0,
            AuditOutcome::Ok,
            None,
            extras,
        );
        let json = serde_json::to_value(envelope).unwrap();
        assert_eq!(
            json["rewrites"],
            serde_json::json!(["ingress:a2a.gateway.planner.message.send -> agent:a2a.agent.planner.message.send"])
        );
        assert!(json.get("stream_consumer").is_none());
    }

    #[test]
    fn forward_ok_audit_includes_stream_consumer_for_message_stream() {
        let agent = test_agent();
        let ingress = "a2a.gateway.planner.message.stream";
        let agent_subject = "a2a.agent.planner.message.stream";
        let extras = forward_audit_fields(ingress, agent_subject, &agent, "message.stream");
        let envelope = AuditEnvelope::new(
            &agent,
            "message/stream",
            None,
            0,
            0,
            AuditOutcome::Ok,
            None,
            extras,
        );
        let json = serde_json::to_value(envelope).unwrap();
        assert_eq!(json["stream_consumer"], "gateway.planner.message.stream");
        assert!(json["rewrites"].is_array());
    }

    #[test]
    fn forward_err_audit_includes_rewrite_for_resubscribe() {
        let agent = test_agent();
        let ingress = "a2a.gateway.planner.tasks.resubscribe";
        let agent_subject = "a2a.agent.planner.tasks.resubscribe";
        let extras = forward_audit_fields(ingress, agent_subject, &agent, "tasks.resubscribe");
        let envelope = AuditEnvelope::new(
            &agent,
            "tasks/resubscribe",
            None,
            0,
            0,
            AuditOutcome::Err {
                code: -32_803,
                message: "gateway failed to publish".into(),
            },
            None,
            extras,
        );
        let json = serde_json::to_value(envelope).unwrap();
        assert_eq!(json["stream_consumer"], "gateway.planner.tasks.resubscribe");
        assert_eq!(
            json["rewrites"],
            serde_json::json!(["ingress:a2a.gateway.planner.tasks.resubscribe -> agent:a2a.agent.planner.tasks.resubscribe"])
        );
    }

    #[test]
    fn policy_denied_audit_omits_rewrite_and_stream_consumer() {
        let agent = test_agent();
        let envelope = AuditEnvelope::new(
            &agent,
            "message/send",
            None,
            0,
            0,
            AuditOutcome::Err {
                code: -32_801,
                message: "tier-2 predicate rejected envelope".into(),
            },
            None,
            denial_audit_fields("deny_guests"),
        );
        let json = serde_json::to_value(envelope).unwrap();
        assert!(json.get("rewrites").is_none());
        assert!(json.get("stream_consumer").is_none());
    }

    #[test]
    fn tier2_cel_denies_message_send_matching_payload() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tier2_dir = dir.path().join("tier2");
        std::fs::create_dir_all(&tier2_dir).expect("tier2 dir");
        std::fs::write(
            tier2_dir.join("deny_guests.cel"),
            r#"request.method == "message/send" && request.params.message.role != "guest""#,
        )
        .expect("write cel");

        let bundle = Tier2CompiledBundle::load_from_dir(&tier2_dir).expect("load bundle");
        let evaluator = RealTier2CelEvaluator::new(bundle);
        let _keep_dir = dir;
        let agent = test_agent();
        let payload = br#"{"jsonrpc":"2.0","id":"1","method":"message/send","params":{"message":{"role":"guest","parts":[]}}}"#;
        let ctx = tier2_evaluation_context_from_ingress(
            "message/send",
            &agent,
            Some("caller-1"),
            &HeaderMap::new(),
            payload,
        );
        let decision = evaluator.evaluate(&ctx);
        assert_eq!(
            decision,
            Tier2Decision::Deny {
                rule: RuleName::new("deny_guests")
            }
        );

        let denied_body = ingress_gateway_policy_denied_response_bytes(payload, "tier-2 predicate rejected envelope")
            .expect("deny response");
        let denied_json: serde_json::Value =
            serde_json::from_slice(&denied_body).expect("deny json");
        assert_eq!(denied_json["error"]["code"], -32_801);

        let envelope = AuditEnvelope::new(
            &agent,
            "message/send",
            Some("1".into()),
            0,
            0,
            AuditOutcome::Err {
                code: -32_801,
                message: "tier-2 predicate rejected envelope".into(),
            },
            Some(payload.as_ref()),
            denial_audit_fields("deny_guests"),
        );
        let audit_json = serde_json::to_value(envelope).expect("audit json");
        assert_eq!(
            audit_json["rules_fired"],
            serde_json::json!(["gateway.tier2.deny_guests"])
        );
    }

    #[test]
    fn tier1_declarative_deny_rule_returns_policy_denied_code() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("deny-planner.tier1.toml"),
            r#"
[[rule]]
id = "deny-planner"
priority = 100
effect = "deny"

[[rule.matches]]
kind = "agent_id"
pattern = "planner"
"#,
        )
        .expect("write bundle");

        let bundle = Tier1DeclarativeBundle::load_from_dir(dir.path()).expect("load bundle");
        let gate = RealTier1DeclarativeGate::new(bundle);
        let agent = test_agent();
        let payload = br#"{"jsonrpc":"2.0","id":"1","method":"message/send","params":{}}"#;
        let ctx = tier1_declarative_context_from_ingress(
            "message.send",
            &agent,
            Some("alice"),
            "a2a",
            "a2a.gateway.planner.message.send",
        )
        .expect("declarative context");
        let decision = gate.evaluate(&ctx);
        assert_eq!(
            decision,
            Tier1DeclarativeDecision::Deny {
                rule: crate::policy::tier1_declarative::Tier1DeclarativeRuleId::new("deny-planner")
            }
        );

        let denied_body = ingress_gateway_policy_denied_response_bytes(
            payload,
            "tier-1 declarative policy rejected envelope",
        )
        .expect("deny response");
        let denied_json: serde_json::Value = serde_json::from_slice(&denied_body).expect("deny json");
        assert_eq!(denied_json["error"]["code"], -32_801);

        let rule_fired = tier1_declarative_audit_rule_fired(&decision);
        assert_eq!(rule_fired, "gateway.tier1.declarative.denied.deny-planner");
    }

    #[test]
    fn tier1_declarative_allow_rule_records_forward_audit_rule() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("allow-planner.tier1.toml"),
            r#"
[[rule]]
id = "allow-planner"
priority = 100
effect = "allow"

[[rule.matches]]
kind = "agent_id"
pattern = "planner"
"#,
        )
        .expect("write bundle");

        let bundle = Tier1DeclarativeBundle::load_from_dir(dir.path()).expect("load bundle");
        let gate = RealTier1DeclarativeGate::new(bundle);
        let agent = test_agent();
        let ctx = tier1_declarative_context_from_ingress(
            "message.send",
            &agent,
            Some("alice"),
            "a2a",
            "a2a.gateway.planner.message.send",
        )
        .expect("declarative context");
        let decision = gate.evaluate(&ctx);
        assert_eq!(
            decision,
            Tier1DeclarativeDecision::Allow {
                rule: Some(crate::policy::tier1_declarative::Tier1DeclarativeRuleId::new(
                    "allow-planner"
                ))
            }
        );
        assert_eq!(
            tier1_declarative_audit_rule_fired(&decision),
            "gateway.tier1.declarative.allowed.allow-planner"
        );
    }
}
