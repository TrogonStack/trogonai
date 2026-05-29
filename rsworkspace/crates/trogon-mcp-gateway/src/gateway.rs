//! Queue-group ingress on `{{prefix}}.gateway.request.>` with backend `request` fan-out.

use std::future::Future;
use std::sync::Arc;

use async_nats::Message;
use async_nats::jetstream;
use bytes::Bytes;
use futures::StreamExt;
use mcp_nats::Config;
use tracing::{Instrument, info, warn};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use trogon_nats::inject_trace_context;

use crate::act_chain::{self, MCP_ACT_CHAIN_HEADER};
use crate::agent_identity::AgentIdentityMode;
use crate::audit::{self, AuditEnvelope, AUDIT_OUTCOME_REDACTED, AUDIT_OUTCOME_REDACTION_SKIPPED};
use crate::authz::{AuthzContext, GatewayIdentity, IdentitySource, PermissionChecker, ToolsListFilterContext};
use crate::egress::{
    EgressMinter, EgressTarget, apply_mesh_egress_headers, scope_for_tools_call, session_id_from_headers,
    strip_inbound_credentials,
};
use crate::ingress::{IngressChainResolve, spawn_schema_cache_invalidation};
use crate::jwt::JwtValidator;
use crate::policy::SpicedbGatePolicy;
use crate::redaction::{
    RedactionApplyResult, RedactionDirection, RedactionOutcome, RedactionRegistry, RewriteEntry, SchemaRedactionContext,
    apply_schema_redaction, merge_outcomes,
};
use crate::rpc_codes;
use crate::schema_cache::{
    SchemaCacheRuntime, ServerId, ensure_tool_schema, lookup_tool_schema, sniff_tools_list_reply,
};
use crate::subject::gateway_to_server_subject;
use crate::throttle::{RateLimitDeny, RateLimiter};
use crate::trace::{DecisionTrace, TraceStore};

const TENANT_HEADER: &str = "trogon-mcp-tenant";
const HEADER_VERIFIED_SUB: &str = "trogon-mcp-verified-sub";
const HEADER_VERIFIED_TENANT: &str = "trogon-mcp-verified-tenant";
const HEADER_IDENTITY_SOURCE: &str = "trogon-mcp-identity-source";
const HEADER_JWT_ISSUER: &str = "trogon-mcp-jwt-issuer";
const MCP_CLIENT_ID_HEADER: &str = "mcp-client-id";
const MCP_SESSION_HEADER: &str = "mcp-session-id";
const AUTHZ_BEARER_PREFIX: &str = "bearer ";
const HEADER_RETRY_AFTER_MS: &str = "retry-after-ms";
const HEADER_RATE_LIMIT_SCOPE: &str = "mcp-rate-limit-scope";
const TRACEPARENT_HEADER: &str = "traceparent";

#[derive(Debug)]
pub struct GatewayError(pub String);

impl std::fmt::Display for GatewayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GatewayError {}

#[derive(Clone)]
pub struct GatewaySettings {
    pub mcp: Config,
    pub queue_group: String,
    pub audit_stream_name: String,
    pub init_audit_stream: bool,
    pub jwt: Arc<JwtValidator>,
    pub egress: Option<Arc<EgressMinter>>,
    pub chain_resolver: Option<Arc<dyn IngressChainResolve>>,
    /// When `None`, Pin 9 defaults apply via in-process limiter (Tier 2 KV sync TODO per ADR 0012).
    pub rate_limit: Option<Arc<RateLimiter>>,
}

fn rate_limiter(settings: &GatewaySettings) -> Arc<RateLimiter> {
    settings
        .rate_limit
        .clone()
        .unwrap_or_else(|| Arc::new(RateLimiter::default()))
}

pub async fn run<S>(
    client: Arc<async_nats::Client>,
    checker: Arc<dyn PermissionChecker>,
    traces: TraceStore,
    settings: GatewaySettings,
    shutdown: S,
) -> Result<(), GatewayError>
where
    S: Future<Output = ()> + Send,
{
    let policy = SpicedbGatePolicy::phase1_hardcoded().map_err(|e| GatewayError(e.to_string()))?;
    let subject = format!("{}.gateway.request.>", settings.mcp.prefix_str());
    let mut subscription = client
        .queue_subscribe(subject.clone(), settings.queue_group.clone())
        .await
        .map_err(|e| GatewayError(e.to_string()))?;

    info!(
        subject = %subject,
        queue_group = %settings.queue_group,
        "subscribed MCP gateway ingress"
    );

    let js = jetstream::new((*client).clone());
    if settings.init_audit_stream
        && let Err(e) = audit::ensure_audit_stream(&js, &settings.audit_stream_name, settings.mcp.prefix_str()).await
    {
        warn!(error = %e, stream = %settings.audit_stream_name, "failed to ensure audit JetStream (continuing without stream guarantee)");
    }

    if let Some(runtime) = SchemaCacheRuntime::shared()
        && let Err(err) = spawn_schema_cache_invalidation(client.clone(), settings.mcp.prefix_str(), runtime).await
    {
        warn!(error = %err, "schema cache invalidation subscribers failed to start");
    }

    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => {
                info!("MCP gateway shutdown signal received");
                break;
            }
            message = subscription.next() => {
                let Some(msg) = message else {
                    break;
                };
                if let Err(e) = handle_ingress(&client, &policy, &checker, &traces, &js, &settings, msg).await {
                    warn!(error = %e, "gateway message handling failed");
                }
            }
        }
    }
    Ok(())
}

async fn handle_ingress(
    client: &async_nats::Client,
    policy: &SpicedbGatePolicy,
    checker: &Arc<dyn PermissionChecker>,
    traces: &TraceStore,
    jetstream: &jetstream::Context,
    settings: &GatewaySettings,
    msg: Message,
) -> Result<(), GatewayError> {
    handle_ingress_inner(client, policy, checker, traces, jetstream, settings, msg.clone())
        .instrument(tracing::info_span!(
            "mcp_gateway.handle_ingress",
            gateway.subject_in = %msg.subject,
            gateway.jsonrpc.method = tracing::field::Empty,
            gateway.identity.source = tracing::field::Empty,
            gateway.identity.issuer_present = tracing::field::Empty,
            gateway.jwt.required_for_gate = tracing::field::Empty,
            gateway.spicedb.required = tracing::field::Empty,
            gateway.spicedb.allowed = tracing::field::Empty,
        ))
        .await
}

#[allow(clippy::too_many_arguments)]
async fn handle_ingress_inner(
    client: &async_nats::Client,
    policy: &SpicedbGatePolicy,
    checker: &Arc<dyn PermissionChecker>,
    traces: &TraceStore,
    jetstream: &jetstream::Context,
    settings: &GatewaySettings,
    msg: Message,
) -> Result<(), GatewayError> {
    let prefix = settings.mcp.prefix_str();
    let backend_subject =
        gateway_to_server_subject(prefix, msg.subject.as_str()).map_err(|e| GatewayError(e.to_string()))?;

    let Some(jsonrpc_method) = jsonrpc_method(&msg.payload) else {
        warn!(subject = %msg.subject, "ingress message has no JSON-RPC method");
        return Ok(());
    };

    let request_id = jsonrpc_request_id(&msg.payload);
    let legacy_tenant_hdr = tenant_from_headers(msg.headers.as_ref());

    let requires_spicedb = policy
        .requires_spicedb_for_method(&jsonrpc_method)
        .map_err(|e| GatewayError(e.to_string()))?;

    tracing::Span::current().record("gateway.jsonrpc.method", jsonrpc_method.as_str());
    tracing::Span::current().record("gateway.spicedb.required", tracing::field::display(requires_spicedb));

    let bearer_h = settings.jwt.bearer_header_name_normalized();
    let bearer = bearer_token_from_headers(msg.headers.as_ref(), bearer_h.as_str());
    let jwt_strict = settings.jwt.jwt_required_for_gate(requires_spicedb);

    tracing::Span::current().record("gateway.jwt.required_for_gate", tracing::field::display(jwt_strict));
    let agent_identity_mode = settings.jwt.agent_identity_mode();
    let act_chain_raw = act_chain::ingress_act_chain_raw(msg.headers.as_ref());
    if let Some(deny) = crate::jwt::enforce_header_act_chain_violations(agent_identity_mode, act_chain_raw.as_deref()) {
        finish_ingress_blocked(FinishIngressBlockedParams {
            client,
            jetstream,
            mcp: &settings.mcp,
            msg: &msg,
            backend_subject: &backend_subject,
            jsonrpc_method: &jsonrpc_method,
            gateway_identity: anonymous_audit_identity(),
            request_id: request_id.clone(),
            requires_spicedb,
            spicedb_allowed: None,
            traces,
            audit_outcome: "error",
            jsonrpc_code: deny.code,
            jsonrpc_message: deny.message,
        })
        .await;
        return Ok(());
    }

    let gateway_resolution = match settings
        .jwt
        .resolve_with_claims(
            bearer.as_deref(),
            legacy_tenant_hdr.as_deref(),
            jwt_strict,
            Some(jsonrpc_method.as_str()),
        )
        .await
    {
        Ok(resolution) => resolution,
        Err(deny) => {
            finish_ingress_blocked(FinishIngressBlockedParams {
                client,
                jetstream,
                mcp: &settings.mcp,
                msg: &msg,
                backend_subject: &backend_subject,
                jsonrpc_method: &jsonrpc_method,
                gateway_identity: anonymous_audit_identity(),
                request_id: request_id.clone(),
                requires_spicedb,
                spicedb_allowed: None,
                traces,
                audit_outcome: "error",
                jsonrpc_code: deny.code,
                jsonrpc_message: deny.message,
            })
            .await;
            return Ok(());
        }
    };
    let gateway_identity = gateway_resolution.identity;
    let jwt_claims = gateway_resolution.claims;

    if let Some(resolver) = settings.chain_resolver.as_ref()
        && let Some(deny) = resolver
            .resolve_inbound_chain(jwt_claims.act_chain.as_deref())
            .await
    {
        finish_ingress_blocked(FinishIngressBlockedParams {
            client,
            jetstream,
            mcp: &settings.mcp,
            msg: &msg,
            backend_subject: &backend_subject,
            jsonrpc_method: &jsonrpc_method,
            gateway_identity: gateway_identity.clone(),
            request_id: request_id.clone(),
            requires_spicedb,
            spicedb_allowed: None,
            traces,
            audit_outcome: "error",
            jsonrpc_code: deny.code,
            jsonrpc_message: deny.message,
        })
        .await;
        return Ok(());
    }

    let span = tracing::Span::current();
    span.record("gateway.identity.source", gateway_identity.source.as_otel_snake_case());
    span.record(
        "gateway.identity.issuer_present",
        tracing::field::display(gateway_identity.issuer.is_some()),
    );
    if let Some(sub) = gateway_identity.caller_sub.as_deref() {
        span.set_attribute("trogon.enduser.id", sub.to_string());
    }
    span.set_attribute(
        "trogon.gateway.identity.source",
        gateway_identity.source.as_otel_snake_case(),
    );

    let tool_call = if jsonrpc_method == "tools/call" {
        tools_call_name(&msg.payload)
    } else {
        None
    };

    let resource_read = if jsonrpc_method == "resources/read" {
        resources_read_uri(&msg.payload)
    } else {
        None
    };

    let server_id = parse_server_id(prefix, msg.subject.as_str()).map_err(|e| GatewayError(e.to_string()))?;
    let session_id = session_id_from_headers(msg.headers.as_ref(), &jwt_claims);
    let server_id_typed = ServerId::new(server_id);
    let client_id = client_id_from_headers(msg.headers.as_ref());
    if let Some(runtime) = SchemaCacheRuntime::shared() {
        runtime.record_client_server(client_id.as_str(), &server_id_typed);
    }

    let tenant_for_rate = gateway_identity
        .tenant
        .as_deref()
        .or(legacy_tenant_hdr.as_deref())
        .unwrap_or("unknown");
    let caller_sub_for_rate = gateway_identity.caller_sub.as_deref().unwrap_or("anonymous");

    let limiter = rate_limiter(settings);
    if let Some(deny) = limiter.check_caller(tenant_for_rate, caller_sub_for_rate) {
        finish_ingress_rate_limited(
            FinishIngressRateLimitedParams {
                client,
                jetstream,
                mcp: &settings.mcp,
                msg: &msg,
                backend_subject: &backend_subject,
                jsonrpc_method: &jsonrpc_method,
                gateway_identity: gateway_identity.clone(),
                request_id: request_id.clone(),
                requires_spicedb,
                spicedb_allowed: None,
                traces,
                deny,
            },
        )
        .await;
        return Ok(());
    }

    let mut spicedb_allowed: Option<bool> = None;
    if requires_spicedb {
        match checker
            .authorize_mcp_request(AuthzContext {
                tenant: gateway_identity.tenant.as_deref(),
                caller_sub: gateway_identity.caller_sub.as_deref(),
                identity_source: gateway_identity.source,
                server_id,
                session_id: Some(session_id.as_str()),
                jsonrpc_method: &jsonrpc_method,
                tool_name: tool_call.as_deref(),
                resource_uri: resource_read.as_deref(),
            })
            .await
        {
            Ok(true) => spicedb_allowed = Some(true),
            Ok(false) => {
                spicedb_allowed = Some(false);
                finish_ingress_blocked(FinishIngressBlockedParams {
                    client,
                    jetstream,
                    mcp: &settings.mcp,
                    msg: &msg,
                    backend_subject: &backend_subject,
                    jsonrpc_method: &jsonrpc_method,
                    gateway_identity: gateway_identity.clone(),
                    request_id: request_id.clone(),
                    requires_spicedb,
                    spicedb_allowed,
                    traces,
                    audit_outcome: "deny",
                    jsonrpc_code: rpc_codes::POLICY_DENY,
                    jsonrpc_message: "policy_deny".to_string(),
                })
                .await;
                return Ok(());
            }
            Err(authz_err) => {
                finish_ingress_blocked(FinishIngressBlockedParams {
                    client,
                    jetstream,
                    mcp: &settings.mcp,
                    msg: &msg,
                    backend_subject: &backend_subject,
                    jsonrpc_method: &jsonrpc_method,
                    gateway_identity: gateway_identity.clone(),
                    request_id: request_id.clone(),
                    requires_spicedb,
                    spicedb_allowed: None,
                    traces,
                    audit_outcome: "error",
                    jsonrpc_code: rpc_codes::AUTHZ_UNREACHABLE,
                    jsonrpc_message: authz_err.0,
                })
                .await;
                return Ok(());
            }
        }
    }

    if requires_spicedb && let Some(allowed) = spicedb_allowed {
        tracing::Span::current().record("gateway.spicedb.allowed", tracing::field::display(allowed));
    }

    if jsonrpc_method == "tools/call"
        && let (Some(runtime), Some(tool)) = (SchemaCacheRuntime::shared(), tool_call.as_deref())
    {
        let _ = ensure_tool_schema(
            &runtime,
            client,
            prefix,
            &server_id_typed,
            tool,
            settings.mcp.operation_timeout(),
        )
        .await
        .map_err(|err| GatewayError(err.to_string()))?;
        let _ = lookup_tool_schema(&runtime, &server_id_typed, tool)
            .await
            .map_err(|err| GatewayError(err.to_string()))?;
    }

    let tenant = gateway_identity
        .tenant
        .as_deref()
        .or(legacy_tenant_hdr.as_deref())
        .unwrap_or("unknown");
    let caller_sub = gateway_identity.caller_sub.as_deref().unwrap_or("anonymous");
    let scope = scope_for_tools_call(server_id, tool_call.as_deref());

    let mesh_token = if let Some(egress) = settings.egress.as_ref() {
        match egress
            .mint(
                agent_identity_mode,
                bearer.as_deref(),
                tenant,
                caller_sub,
                session_id.as_str(),
                scope.as_deref(),
                jwt_claims.purpose.as_deref(),
                EgressTarget::Backend {
                    server_id: server_id.to_string(),
                },
            )
            .await
        {
            Ok(token) => token,
            Err(err) => {
                finish_ingress_blocked(FinishIngressBlockedParams {
                    client,
                    jetstream,
                    mcp: &settings.mcp,
                    msg: &msg,
                    backend_subject: &backend_subject,
                    jsonrpc_method: &jsonrpc_method,
                    gateway_identity: gateway_identity.clone(),
                    request_id: request_id.clone(),
                    requires_spicedb,
                    spicedb_allowed,
                    traces,
                    audit_outcome: "error",
                    jsonrpc_code: err.code,
                    jsonrpc_message: err.message,
                })
                .await;
                return Ok(());
            }
        }
    } else {
        None
    };

    let base_headers = msg.headers.clone().unwrap_or_default();
    let mut outbound_headers = egress_header_map(base_headers, settings.jwt.jwt_controls_transport());

    let mesh_enforce = mesh_token.is_some() && agent_identity_mode == AgentIdentityMode::Enforce;
    let mesh_shadow = mesh_token.is_some() && agent_identity_mode == AgentIdentityMode::Shadow;

    if mesh_enforce {
        strip_inbound_credentials(&mut outbound_headers, bearer_h.as_str());
    } else if settings.jwt.jwt_controls_transport() {
        append_verified_gateway_identity_headers(&mut outbound_headers, &gateway_identity);
    }

    if !mesh_enforce {
        act_chain::project_act_chain_header(&mut outbound_headers, act_chain_raw.as_deref(), agent_identity_mode);
    }

    if let Some(ref token) = mesh_token {
        apply_mesh_egress_headers(
            &mut outbound_headers,
            token,
            agent_identity_mode,
            bearer_h.as_str(),
            mesh_shadow,
        );
    }

    inject_trace_context(&mut outbound_headers);

    let _inflight_guard = match limiter.try_acquire_inflight(server_id, tenant_for_rate) {
        Ok(guard) => guard,
        Err(deny) => {
            finish_ingress_rate_limited(FinishIngressRateLimitedParams {
                client,
                jetstream,
                mcp: &settings.mcp,
                msg: &msg,
                backend_subject: &backend_subject,
                jsonrpc_method: &jsonrpc_method,
                gateway_identity: gateway_identity.clone(),
                request_id: request_id.clone(),
                requires_spicedb,
                spicedb_allowed,
                traces,
                deny,
            })
            .await;
            return Ok(());
        }
    };

    let tenant_for_redaction = gateway_identity
        .tenant
        .as_deref()
        .or(legacy_tenant_hdr.as_deref());
    let (forward_payload, request_redaction, request_redaction_skip) = redact_tools_call_payload(
        server_id,
        tool_call.as_deref(),
        &jsonrpc_method,
        tenant_for_redaction,
        RedactionDirection::Request,
        &msg.payload,
    )
    .await?;

    if let Some(reason) = request_redaction_skip.as_ref() {
        warn!(
            server_id = %server_id,
            tool = tool_call.as_deref().unwrap_or(""),
            reason = %reason,
            "schema-driven redaction skipped on request"
        );
        publish_redaction_skipped_audit(
            jetstream,
            prefix,
            "request",
            &msg.subject,
            &backend_subject,
            &jsonrpc_method,
            &gateway_identity,
            request_id.clone(),
            reason,
        )
        .await;
    } else if !request_redaction.rewrites.is_empty() {
        publish_redaction_rule_audits(
            jetstream,
            prefix,
            "request",
            &msg.subject,
            &backend_subject,
            &jsonrpc_method,
            &gateway_identity,
            request_id.clone(),
            &request_redaction.rewrites,
        )
        .await;
    }

    if msg.reply.is_none() {
        client
            .publish_with_headers(backend_subject.clone(), outbound_headers, forward_payload)
            .await
            .map_err(|e| GatewayError(e.to_string()))?;
        client.flush().await.map_err(|e| GatewayError(e.to_string()))?;

        publish_allow_audit_and_maybe_trace_no_reply(
            jetstream,
            prefix,
            &msg,
            &backend_subject,
            &jsonrpc_method,
            &gateway_identity,
            request_id.clone(),
            &request_redaction,
        )
        .await;
        return Ok(());
    }

    let timeout = settings.mcp.operation_timeout();
    let backend_result = tokio::time::timeout(
        timeout,
        client.request_with_headers(backend_subject.clone(), outbound_headers, forward_payload),
    )
    .await;

    let outcome: &'static str = match &backend_result {
        Ok(Ok(_)) => "allow",
        Ok(Err(_)) => "error",
        Err(_) => "error",
    };

    let mut combined_redaction = request_redaction;
    let mut redacted_response_payload: Option<Bytes> = None;
    if let Ok(Ok(response)) = &backend_result {
        let (response_payload, response_redaction, response_skip) = redact_tools_call_payload(
            server_id,
            tool_call.as_deref(),
            &jsonrpc_method,
            tenant_for_redaction,
            RedactionDirection::Response,
            &response.payload,
        )
        .await?;
        if let Some(reason) = response_skip.as_ref() {
            warn!(
                server_id = %server_id,
                tool = tool_call.as_deref().unwrap_or(""),
                reason = %reason,
                "schema-driven redaction skipped on response"
            );
            publish_redaction_skipped_audit(
                jetstream,
                prefix,
                "response",
                &msg.subject,
                &backend_subject,
                &jsonrpc_method,
                &gateway_identity,
                request_id.clone(),
                reason,
            )
            .await;
            redacted_response_payload = Some(response.payload.clone());
        } else {
            if !response_redaction.rewrites.is_empty() {
                publish_redaction_rule_audits(
                    jetstream,
                    prefix,
                    "response",
                    &msg.subject,
                    &backend_subject,
                    &jsonrpc_method,
                    &gateway_identity,
                    request_id.clone(),
                    &response_redaction.rewrites,
                )
                .await;
            }
            combined_redaction = merge_outcomes(combined_redaction, response_redaction);
            redacted_response_payload = Some(response_payload);
        }
    }

    publish_audit_inner(
        jetstream,
        prefix,
        outcome,
        "request",
        &msg.subject,
        &backend_subject,
        &jsonrpc_method,
        &gateway_identity,
        request_id.clone(),
        &combined_redaction,
    )
    .await;

    if let Some(id) = &request_id {
        traces.insert(
            id.to_string(),
            DecisionTrace {
                subject_in: msg.subject.to_string(),
                subject_out: backend_subject.clone(),
                jsonrpc_method: jsonrpc_method.clone(),
                cel_requires_spicedb: requires_spicedb,
                spicedb_allowed,
                tenant: gateway_identity.tenant.clone(),
                caller_sub: gateway_identity.caller_sub.clone(),
                identity_source: gateway_identity.source,
            },
        );
    }

    match backend_result {
        Ok(Ok(response)) => {
            if jsonrpc_method == "tools/list"
                && let Some(runtime) = SchemaCacheRuntime::shared()
            {
                if let Err(err) = sniff_tools_list_reply(
                    &runtime.cache,
                    &runtime.config,
                    &server_id_typed,
                    &response.payload,
                )
                .await
                {
                    warn!(error = %err, server_id = %server_id, "tools/list schema sniff failed");
                }
            }
            let payload = if jsonrpc_method == "tools/list" {
                shape_tools_list_response(
                    checker.as_ref(),
                    &gateway_identity,
                    server_id,
                    session_id.as_str(),
                    response.payload,
                )
                .await?
            } else {
                redacted_response_payload.unwrap_or(response.payload)
            };
            dispatch_backend_response(client, &msg, payload).await?;
        }
        Ok(Err(e)) => {
            if let Some(runtime) = SchemaCacheRuntime::shared()
                && let Err(err) = runtime.invalidate_on_reconnect(&server_id_typed).await
            {
                warn!(error = %err, server_id = %server_id, "schema cache reconnect invalidation failed");
            }
            respond_with_jsonrpc_error(
                client,
                &msg,
                request_id,
                rpc_codes::BACKEND_UNREACHABLE,
                format!("upstream request failed: {e}"),
            )
            .await?;
        }
        Err(_) => {
            respond_with_jsonrpc_error(
                client,
                &msg,
                request_id,
                rpc_codes::BACKEND_TIMEOUT,
                "upstream request timed out".to_string(),
            )
            .await?;
        }
    }

    Ok(())
}

fn anonymous_audit_identity() -> GatewayIdentity {
    GatewayIdentity {
        tenant: None,
        caller_sub: None,
        issuer: None,
        jti: None,
        source: IdentitySource::Anonymous,
    }
}

async fn publish_allow_audit_and_maybe_trace_no_reply(
    jetstream: &jetstream::Context,
    prefix: &str,
    msg: &Message,
    backend_subject: &str,
    jsonrpc_method: &str,
    gateway_identity: &GatewayIdentity,
    request_id: Option<serde_json::Value>,
    redaction: &RedactionOutcome,
) {
    publish_audit_inner(
        jetstream,
        prefix,
        "allow",
        "request",
        &msg.subject,
        backend_subject,
        jsonrpc_method,
        gateway_identity,
        request_id,
        redaction,
    )
    .await;
}

#[allow(clippy::too_many_arguments)] // Audit publish mirrors many gateway fields; struct would add noise here.
async fn publish_audit_inner(
    jetstream: &jetstream::Context,
    prefix: &str,
    outcome: &'static str,
    direction: &'static str,
    subject_in: &async_nats::Subject,
    subject_out: &str,
    jsonrpc_method: &str,
    gateway_identity: &GatewayIdentity,
    request_id: Option<serde_json::Value>,
    redaction: &RedactionOutcome,
) {
    let mut audit_envelope = AuditEnvelope::new(
        subject_in.to_string(),
        subject_out.to_string(),
        outcome,
        direction,
        jsonrpc_method.to_string(),
        gateway_identity.tenant.clone(),
        gateway_identity.caller_sub.clone(),
        gateway_identity.issuer.clone(),
        gateway_identity.source,
        request_id,
        None,
    );
    audit_envelope.apply_rewrites(&redaction.rewrites);
    let method_root = audit::jsonrpc_method_root(jsonrpc_method);
    let audit_subject = audit::audit_publish_subject(prefix, outcome, direction, &method_root);
    audit::publish_audit(
        jetstream,
        audit_subject,
        &audit_envelope,
        std::time::Duration::from_secs(5),
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn publish_redaction_rule_audits(
    jetstream: &jetstream::Context,
    prefix: &str,
    direction: &'static str,
    subject_in: &async_nats::Subject,
    subject_out: &str,
    jsonrpc_method: &str,
    gateway_identity: &GatewayIdentity,
    request_id: Option<serde_json::Value>,
    rewrites: &[RewriteEntry],
) {
    for rewrite in rewrites {
        let mut envelope = AuditEnvelope::new(
            subject_in.to_string(),
            subject_out.to_string(),
            AUDIT_OUTCOME_REDACTED,
            direction,
            jsonrpc_method.to_string(),
            gateway_identity.tenant.clone(),
            gateway_identity.caller_sub.clone(),
            gateway_identity.issuer.clone(),
            gateway_identity.source,
            request_id.clone(),
            None,
        );
        envelope.apply_rewrites(std::slice::from_ref(rewrite));
        let method_root = audit::jsonrpc_method_root(jsonrpc_method);
        let audit_subject =
            audit::audit_publish_subject(prefix, AUDIT_OUTCOME_REDACTED, direction, &method_root);
        audit::publish_audit(
            jetstream,
            audit_subject,
            &envelope,
            std::time::Duration::from_secs(5),
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn publish_redaction_skipped_audit(
    jetstream: &jetstream::Context,
    prefix: &str,
    direction: &'static str,
    subject_in: &async_nats::Subject,
    subject_out: &str,
    jsonrpc_method: &str,
    gateway_identity: &GatewayIdentity,
    request_id: Option<serde_json::Value>,
    reason: &str,
) {
    let mut envelope = AuditEnvelope::new(
        subject_in.to_string(),
        subject_out.to_string(),
        AUDIT_OUTCOME_REDACTION_SKIPPED,
        direction,
        jsonrpc_method.to_string(),
        gateway_identity.tenant.clone(),
        gateway_identity.caller_sub.clone(),
        gateway_identity.issuer.clone(),
        gateway_identity.source,
        request_id,
        None,
    );
    envelope.apply_redaction_skip_reason(reason);
    let method_root = audit::jsonrpc_method_root(jsonrpc_method);
    let audit_subject = audit::audit_publish_subject(
        prefix,
        AUDIT_OUTCOME_REDACTION_SKIPPED,
        direction,
        &method_root,
    );
    audit::publish_audit(
        jetstream,
        audit_subject,
        &envelope,
        std::time::Duration::from_secs(5),
    )
    .await;
}

async fn redact_tools_call_payload(
    server_id: &str,
    tool_name: Option<&str>,
    jsonrpc_method: &str,
    tenant: Option<&str>,
    direction: RedactionDirection,
    payload: &Bytes,
) -> Result<(Bytes, RedactionOutcome, Option<String>), GatewayError> {
    if jsonrpc_method != "tools/call" {
        return Ok((payload.clone(), RedactionOutcome::empty(), None));
    }
    let Some(tool_name) = tool_name else {
        return Ok((payload.clone(), RedactionOutcome::empty(), None));
    };

    let mut doc: serde_json::Value =
        serde_json::from_slice(payload).map_err(|e| GatewayError(format!("jsonrpc payload decode: {e}")))?;
    let ctx = SchemaRedactionContext {
        server_id,
        tool_name,
        direction,
        hash_salt: tenant,
    };
    match apply_schema_redaction(
        SchemaCacheRuntime::shared().as_deref(),
        RedactionRegistry::shared().as_deref(),
        ctx,
        &mut doc,
    )
    .await
    {
        RedactionApplyResult::Passthrough => Ok((payload.clone(), RedactionOutcome::empty(), None)),
        RedactionApplyResult::Skipped { reason } => Ok((payload.clone(), RedactionOutcome::empty(), Some(reason))),
        RedactionApplyResult::Applied(outcome) => {
            let bytes =
                serde_json::to_vec(&doc).map_err(|e| GatewayError(format!("jsonrpc payload encode: {e}")))?;
            Ok((Bytes::from(bytes), outcome, None))
        }
    }
}

struct FinishIngressBlockedParams<'a> {
    client: &'a async_nats::Client,
    jetstream: &'a jetstream::Context,
    mcp: &'a Config,
    msg: &'a Message,
    backend_subject: &'a str,
    jsonrpc_method: &'a str,
    gateway_identity: GatewayIdentity,
    request_id: Option<serde_json::Value>,
    requires_spicedb: bool,
    spicedb_allowed: Option<bool>,
    traces: &'a TraceStore,
    audit_outcome: &'static str,
    jsonrpc_code: i32,
    jsonrpc_message: String,
}

async fn finish_ingress_rate_limited(params: FinishIngressRateLimitedParams<'_>) {
    let prefix = params.mcp.prefix_str();
    let trace_id = trace_id_from_headers(params.msg.headers.as_ref());
    if params.msg.reply.is_some() {
        reply_with_rate_limited_error(
            params.client,
            params.msg,
            params.request_id.clone(),
            &trace_id,
            &params.deny,
        )
        .await;
    }
    let mut envelope = AuditEnvelope::new(
        params.msg.subject.to_string(),
        params.backend_subject.to_string(),
        "rate_limited",
        "request",
        params.jsonrpc_method.to_string(),
        params.gateway_identity.tenant.clone(),
        params.gateway_identity.caller_sub.clone(),
        params.gateway_identity.issuer.clone(),
        params.gateway_identity.source,
        params.request_id.clone(),
        None,
    );
    envelope.apply_rate_limit_fields(params.deny.scope.as_str(), params.deny.retry_after_ms);
    let method_root = audit::jsonrpc_method_root(params.jsonrpc_method);
    let subject = audit::audit_publish_subject(prefix, "rate_limited", "request", &method_root);
    audit::publish_audit(params.jetstream, subject, &envelope, std::time::Duration::from_secs(5)).await;

    if let Some(id) = params.request_id {
        params.traces.insert(
            id.to_string(),
            DecisionTrace {
                subject_in: params.msg.subject.to_string(),
                subject_out: params.backend_subject.to_string(),
                jsonrpc_method: params.jsonrpc_method.to_string(),
                cel_requires_spicedb: params.requires_spicedb,
                spicedb_allowed: params.spicedb_allowed,
                tenant: params.gateway_identity.tenant,
                caller_sub: params.gateway_identity.caller_sub,
                identity_source: params.gateway_identity.source,
            },
        );
    }
}

struct FinishIngressRateLimitedParams<'a> {
    client: &'a async_nats::Client,
    jetstream: &'a jetstream::Context,
    mcp: &'a Config,
    msg: &'a Message,
    backend_subject: &'a str,
    jsonrpc_method: &'a str,
    gateway_identity: GatewayIdentity,
    request_id: Option<serde_json::Value>,
    requires_spicedb: bool,
    spicedb_allowed: Option<bool>,
    traces: &'a TraceStore,
    deny: RateLimitDeny,
}

async fn finish_ingress_blocked(params: FinishIngressBlockedParams<'_>) {
    let prefix = params.mcp.prefix_str();
    if params.msg.reply.is_some() {
        reply_with_jsonrpc_error(
            params.client,
            params.msg,
            params.request_id.clone(),
            params.jsonrpc_code,
            params.jsonrpc_message.clone(),
        )
        .await;
    }
    let envelope = AuditEnvelope::new(
        params.msg.subject.to_string(),
        params.backend_subject.to_string(),
        params.audit_outcome,
        "request",
        params.jsonrpc_method.to_string(),
        params.gateway_identity.tenant.clone(),
        params.gateway_identity.caller_sub.clone(),
        params.gateway_identity.issuer.clone(),
        params.gateway_identity.source,
        params.request_id.clone(),
        None,
    );
    let method_root = audit::jsonrpc_method_root(params.jsonrpc_method);
    let subject = audit::audit_publish_subject(prefix, params.audit_outcome, "request", &method_root);
    audit::publish_audit(params.jetstream, subject, &envelope, std::time::Duration::from_secs(5)).await;

    if let Some(id) = params.request_id {
        params.traces.insert(
            id.to_string(),
            DecisionTrace {
                subject_in: params.msg.subject.to_string(),
                subject_out: params.backend_subject.to_string(),
                jsonrpc_method: params.jsonrpc_method.to_string(),
                cel_requires_spicedb: params.requires_spicedb,
                spicedb_allowed: params.spicedb_allowed,
                tenant: params.gateway_identity.tenant,
                caller_sub: params.gateway_identity.caller_sub,
                identity_source: params.gateway_identity.source,
            },
        );
    }
}

fn jsonrpc_method(payload: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(payload).ok()?;
    Some(value.get("method")?.as_str()?.to_string())
}

fn jsonrpc_request_id(payload: &[u8]) -> Option<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_slice(payload).ok()?;
    value.get("id").cloned()
}

fn tools_call_name(payload: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(payload).ok()?;
    value
        .get("params")?
        .get("name")?
        .as_str()
        .map(std::string::ToString::to_string)
}

fn resources_read_uri(payload: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(payload).ok()?;
    value
        .get("params")?
        .get("uri")?
        .as_str()
        .map(std::string::ToString::to_string)
}

fn tenant_from_headers(headers: Option<&async_nats::HeaderMap>) -> Option<String> {
    let h = headers?;
    Some(
        h.get_last(TENANT_HEADER)
            .or_else(|| h.get(TENANT_HEADER))?
            .as_str()
            .to_string(),
    )
}

fn client_id_from_headers(headers: Option<&async_nats::HeaderMap>) -> String {
    let Some(h) = headers else {
        return "gateway-default".to_string();
    };
    if let Some(id) = h
        .get_last(MCP_CLIENT_ID_HEADER)
        .or_else(|| h.get(MCP_CLIENT_ID_HEADER))
        .map(|v| v.as_str().to_string())
        .filter(|id| !id.is_empty())
    {
        return id;
    }
    h.get_last(MCP_SESSION_HEADER)
        .or_else(|| h.get(MCP_SESSION_HEADER))
        .map(|v| v.as_str().to_string())
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| "gateway-default".to_string())
}

fn bearer_token_from_headers(headers: Option<&async_nats::HeaderMap>, header_name_normalized: &str) -> Option<String> {
    let hm = headers?;
    let hv = hm
        .get_last(header_name_normalized)
        .or_else(|| hm.get(header_name_normalized))?;
    bearer_from_authorization_header_value(hv.as_str())
}

fn bearer_from_authorization_header_value(raw: &str) -> Option<String> {
    let s = raw.trim();
    let plen = AUTHZ_BEARER_PREFIX.len();
    if s.len() >= plen && s[..plen].eq_ignore_ascii_case(AUTHZ_BEARER_PREFIX) {
        let tok = s[plen..].trim();
        if tok.is_empty() { None } else { Some(tok.to_string()) }
    } else {
        None
    }
}

fn append_verified_gateway_identity_headers(headers: &mut async_nats::HeaderMap, identity: &GatewayIdentity) {
    if identity.source != IdentitySource::Jwt {
        return;
    }
    if let Some(ref sub) = identity.caller_sub {
        headers.insert(HEADER_VERIFIED_SUB, sub.as_str());
    }
    if let Some(ref tenant) = identity.tenant {
        headers.insert(HEADER_VERIFIED_TENANT, tenant.as_str());
    }
    headers.insert(HEADER_IDENTITY_SOURCE, identity.source.as_otel_snake_case());
    if let Some(ref iss) = identity.issuer {
        headers.insert(HEADER_JWT_ISSUER, iss.as_str());
    }
}

fn egress_header_map(src: async_nats::HeaderMap, strip_legacy_tenant: bool) -> async_nats::HeaderMap {
    let mut out = async_nats::HeaderMap::new();
    for (name, vals) in src.iter() {
        let header_name_ref: &str = AsRef::<str>::as_ref(name);
        if header_name_ref.eq_ignore_ascii_case(MCP_ACT_CHAIN_HEADER) {
            continue;
        }
        if strip_legacy_tenant && header_name_ref.eq_ignore_ascii_case(TENANT_HEADER) {
            continue;
        }
        for v in vals {
            out.append(name.clone(), v.clone());
        }
    }
    out
}

fn parse_server_id<'a>(prefix: &str, subject: &'a str) -> Result<&'a str, &'static str> {
    let head = format!("{prefix}.gateway.request.");
    let rest = subject
        .strip_prefix(head.as_str())
        .ok_or("subject missing gateway.request prefix")?;
    let (server, _) = rest.split_once('.').ok_or("gateway subject missing server id")?;
    if server.is_empty() {
        return Err("empty server id");
    }
    Ok(server)
}

async fn shape_tools_list_response(
    checker: &dyn PermissionChecker,
    gateway_identity: &GatewayIdentity,
    server_id: &str,
    session_id: &str,
    payload: Bytes,
) -> Result<Bytes, GatewayError> {
    let mut value: serde_json::Value =
        serde_json::from_slice(&payload).map_err(|e| GatewayError(format!("tools/list response decode: {e}")))?;
    let Some(tools) = value
        .pointer_mut("/result/tools")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return Ok(payload);
    };

    let tool_names: Vec<String> = tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(serde_json::Value::as_str).map(str::to_string))
        .collect();

    let allowed = checker
        .filter_tools_list(
            ToolsListFilterContext {
                tenant: gateway_identity.tenant.as_deref(),
                caller_sub: gateway_identity.caller_sub.as_deref(),
                identity_source: gateway_identity.source,
                server_id,
                session_id,
            },
            &tool_names,
        )
        .await
        .map_err(|e| GatewayError(e.0))?;

    let allowed_set: std::collections::HashSet<String> = allowed.into_iter().collect();
    tools.retain(|tool| {
        tool.get("name")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|name| allowed_set.contains(name))
    });

    let shaped = serde_json::to_vec(&value).map_err(|e| GatewayError(format!("tools/list response encode: {e}")))?;
    Ok(Bytes::from(shaped))
}

async fn dispatch_backend_response(
    client: &async_nats::Client,
    ingress: &Message,
    payload: Bytes,
) -> Result<(), GatewayError> {
    let Some(reply) = ingress.reply.clone() else {
        return Err(GatewayError("missing reply subject for JSON-RPC request path".into()));
    };
    client
        .publish_with_headers(reply.to_string(), ingress.headers.clone().unwrap_or_default(), payload)
        .await
        .map_err(|e| GatewayError(e.to_string()))?;
    client.flush().await.map_err(|e| GatewayError(e.to_string()))?;
    Ok(())
}

async fn respond_with_jsonrpc_error(
    client: &async_nats::Client,
    ingress: &Message,
    id: Option<serde_json::Value>,
    code: i32,
    message: String,
) -> Result<(), GatewayError> {
    reply_with_jsonrpc_error(client, ingress, id, code, message).await;
    Ok(())
}

async fn reply_with_rate_limited_error(
    client: &async_nats::Client,
    ingress: &Message,
    id: Option<serde_json::Value>,
    trace_id: &str,
    deny: &RateLimitDeny,
) {
    let body = rate_limited_error_bytes(id, trace_id, deny);
    let Some(reply) = ingress.reply.clone() else {
        warn!("cannot send JSON-RPC rate_limited: ingress message has no reply subject");
        return;
    };
    let mut headers = async_nats::HeaderMap::new();
    let retry_ms = deny.retry_after_ms.to_string();
    headers.insert(HEADER_RETRY_AFTER_MS, retry_ms.as_str());
    headers.insert(HEADER_RATE_LIMIT_SCOPE, deny.scope.as_str());
    if let Err(e) = client
        .publish_with_headers(reply.to_string(), headers, body)
        .await
    {
        warn!(error = %e, "failed to publish rate_limited JSON-RPC to reply subject");
    }
    if let Err(e) = client.flush().await {
        warn!(error = %e, "flush after rate_limited JSON-RPC failed");
    }
}

fn rate_limited_error_bytes(id: Option<serde_json::Value>, trace_id: &str, deny: &RateLimitDeny) -> Bytes {
    let value = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": rpc_codes::RATE_LIMITED,
            "message": "rate_limited",
            "data": {
                "trace_id": trace_id,
                "scope": deny.scope.as_str(),
                "retry_after_ms": deny.retry_after_ms,
            }
        }
    });
    Bytes::from(value.to_string())
}

fn trace_id_from_headers(headers: Option<&async_nats::HeaderMap>) -> String {
    if let Some(h) = headers {
        if let Some(tp) = h
            .get_last(TRACEPARENT_HEADER)
            .or_else(|| h.get(TRACEPARENT_HEADER))
        {
            let parts: Vec<&str> = tp.as_str().split('-').collect();
            if parts.len() >= 3 && parts[1].len() == 32 {
                return parts[1].to_string();
            }
        }
    }
    format!(
        "{:032x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u128
    )
}

async fn reply_with_jsonrpc_error(
    client: &async_nats::Client,
    ingress: &Message,
    id: Option<serde_json::Value>,
    code: i32,
    message: String,
) {
    let body = jsonrpc_error_bytes(id, code, message);
    let Some(reply) = ingress.reply.clone() else {
        warn!("cannot send JSON-RPC error: ingress message has no reply subject");
        return;
    };
    if let Err(e) = client
        .publish_with_headers(reply.to_string(), async_nats::HeaderMap::new(), body)
        .await
    {
        warn!(error = %e, "failed to publish JSON-RPC error to reply subject");
    }
    if let Err(e) = client.flush().await {
        warn!(error = %e, "flush after JSON-RPC error failed");
    }
}

fn jsonrpc_error_bytes(id: Option<serde_json::Value>, code: i32, message: String) -> Bytes {
    let value = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    });
    Bytes::from(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resources_read_uri_parses_params() {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "resources/read",
            "params": {"uri": "file:///tmp/x"}
        })
        .to_string();
        assert_eq!(resources_read_uri(payload.as_bytes()).as_deref(), Some("file:///tmp/x"));
    }

    #[test]
    fn parse_server_id_ok() {
        assert_eq!(
            parse_server_id("mcp", "mcp.gateway.request.filesystem.tools.call").unwrap(),
            "filesystem"
        );
    }

    #[test]
    fn strips_tenant_header_when_configured() {
        let mut h = async_nats::HeaderMap::new();
        h.insert(TENANT_HEADER, "evil");
        h.insert("X-Other", "v");
        let preserved = egress_header_map(h.clone(), false);
        assert!(preserved.get(TENANT_HEADER).is_some());
        let stripped = egress_header_map(h, true);
        assert!(stripped.get(TENANT_HEADER).is_none());
        assert!(stripped.get("X-Other").is_some());
    }

    #[test]
    fn always_strips_act_chain_header_from_ingress() {
        let mut h = async_nats::HeaderMap::new();
        h.insert(MCP_ACT_CHAIN_HEADER, r#"[{"sub":"evil","iat":1}]"#);
        h.insert("X-Other", "v");
        let out = egress_header_map(h, false);
        assert!(out.get(MCP_ACT_CHAIN_HEADER).is_none());
        assert!(out.get("X-Other").is_some());
    }

    #[test]
    fn bearer_parses_case_insensitive() {
        assert_eq!(
            bearer_from_authorization_header_value("Bearer abc.def").as_deref(),
            Some("abc.def")
        );
    }

    #[test]
    fn trace_id_from_traceparent_header() {
        let mut h = async_nats::HeaderMap::new();
        h.insert(
            TRACEPARENT_HEADER,
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        );
        assert_eq!(
            trace_id_from_headers(Some(&h)),
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
    }

    #[test]
    fn rate_limited_error_bytes_include_scope_and_retry_after_ms() {
        let deny = crate::throttle::RateLimitDeny {
            scope: crate::throttle::RateLimitScope::Caller,
            retry_after_ms: 750,
        };
        let body = rate_limited_error_bytes(Some(serde_json::json!(1)), "abc123", &deny);
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(value["error"]["code"], rpc_codes::RATE_LIMITED);
        assert_eq!(value["error"]["message"], "rate_limited");
        assert_eq!(value["error"]["data"]["scope"], "caller");
        assert_eq!(value["error"]["data"]["retry_after_ms"], 750);
        assert_eq!(value["error"]["data"]["trace_id"], "abc123");
    }
}
