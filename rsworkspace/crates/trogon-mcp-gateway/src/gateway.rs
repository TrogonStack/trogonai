//! Queue-group ingress on `{{prefix}}.gateway.request.>` with backend `request` fan-out.

use std::future::Future;
use std::sync::Arc;

use async_nats::jetstream;
use async_nats::Message;
use bytes::Bytes;
use futures::StreamExt;
use mcp_nats::Config;
use tracing::{info, warn};
use trogon_nats::inject_trace_context;

use crate::audit::{self, AuditEnvelope};
use crate::authz::{AuthzContext, PermissionChecker};
use crate::policy::SpicedbGatePolicy;
use crate::rpc_codes;
use crate::subject::gateway_to_server_subject;
use crate::trace::{DecisionTrace, TraceStore};

const TENANT_HEADER: &str = "trogon-mcp-tenant";

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
        && let Err(e) =
            audit::ensure_audit_stream(&js, &settings.audit_stream_name, settings.mcp.prefix_str()).await
    {
        warn!(error = %e, stream = %settings.audit_stream_name, "failed to ensure audit JetStream (continuing without stream guarantee)");
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
                if let Err(e) = handle_ingress(
                    &client,
                    &policy,
                    &checker,
                    &traces,
                    &js,
                    &settings.mcp,
                    msg,
                )
                .await
                {
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
    mcp: &Config,
    msg: Message,
) -> Result<(), GatewayError> {
    let prefix = mcp.prefix_str();
    let backend_subject = gateway_to_server_subject(prefix, msg.subject.as_str()).map_err(|e| GatewayError(e.to_string()))?;

    let Some(jsonrpc_method) = jsonrpc_method(&msg.payload) else {
        warn!(subject = %msg.subject, "ingress message has no JSON-RPC method");
        return Ok(());
    };

    let request_id = jsonrpc_request_id(&msg.payload);
    let tenant = tenant_from_headers(msg.headers.as_ref());

    let requires_spicedb = policy
        .requires_spicedb_for_method(&jsonrpc_method)
        .map_err(|e| GatewayError(e.to_string()))?;

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

    let mut spicedb_allowed: Option<bool> = None;
    if requires_spicedb {
        match checker
            .authorize_mcp_request(AuthzContext {
                tenant: tenant.as_deref(),
                server_id,
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
                    mcp,
                    msg: &msg,
                    backend_subject: &backend_subject,
                    jsonrpc_method: &jsonrpc_method,
                    tenant: tenant.clone(),
                    request_id: request_id.clone(),
                    requires_spicedb,
                    spicedb_allowed,
                    audit_outcome: "deny",
                    jsonrpc_code: rpc_codes::POLICY_DENY,
                    jsonrpc_message: "policy_deny".to_string(),
                    traces,
                })
                .await;
                return Ok(());
            }
            Err(authz_err) => {
                finish_ingress_blocked(FinishIngressBlockedParams {
                    client,
                    jetstream,
                    mcp,
                    msg: &msg,
                    backend_subject: &backend_subject,
                    jsonrpc_method: &jsonrpc_method,
                    tenant: tenant.clone(),
                    request_id: request_id.clone(),
                    requires_spicedb,
                    spicedb_allowed: None,
                    audit_outcome: "error",
                    jsonrpc_code: rpc_codes::AUTHZ_UNREACHABLE,
                    jsonrpc_message: authz_err.0,
                    traces,
                })
                .await;
                return Ok(());
            }
        }
    }

    let mut outbound_headers = msg.headers.clone().unwrap_or_default();
    inject_trace_context(&mut outbound_headers);

    if msg.reply.is_none() {
        client
            .publish_with_headers(
                backend_subject.clone(),
                outbound_headers,
                msg.payload.clone(),
            )
            .await
            .map_err(|e| GatewayError(e.to_string()))?;
        client.flush().await.map_err(|e| GatewayError(e.to_string()))?;

        let audit_envelope = AuditEnvelope {
            subject_in: msg.subject.to_string(),
            subject_out: backend_subject.clone(),
            outcome: "allow",
            direction: "request",
            jsonrpc_method: jsonrpc_method.clone(),
            tenant: tenant.clone(),
            request_id: request_id.clone(),
        };
        let method_root = audit::jsonrpc_method_root(&jsonrpc_method);
        let audit_subject = audit::audit_publish_subject(prefix, "allow", "request", &method_root);
        audit::publish_audit(
            jetstream,
            audit_subject,
            &audit_envelope,
            std::time::Duration::from_secs(5),
        )
        .await;
        return Ok(());
    }

    let timeout = mcp.operation_timeout();
    let backend_result = tokio::time::timeout(
        timeout,
        client.request_with_headers(backend_subject.clone(), outbound_headers, msg.payload.clone()),
    )
    .await;

    let outcome: &'static str = match &backend_result {
        Ok(Ok(_)) => "allow",
        Ok(Err(_)) => "error",
        Err(_) => "error",
    };

    let audit_envelope = AuditEnvelope {
        subject_in: msg.subject.to_string(),
        subject_out: backend_subject.clone(),
        outcome,
        direction: "request",
        jsonrpc_method: jsonrpc_method.clone(),
        tenant: tenant.clone(),
        request_id: request_id.clone(),
    };
    let method_root = audit::jsonrpc_method_root(&jsonrpc_method);
    let audit_subject = audit::audit_publish_subject(prefix, outcome, "request", &method_root);
    audit::publish_audit(
        jetstream,
        audit_subject,
        &audit_envelope,
        std::time::Duration::from_secs(5),
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
            },
        );
    }

    match backend_result {
        Ok(Ok(response)) => {
            dispatch_backend_response(client, &msg, response.payload).await?;
        }
        Ok(Err(e)) => {
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

struct FinishIngressBlockedParams<'a> {
    client: &'a async_nats::Client,
    jetstream: &'a jetstream::Context,
    mcp: &'a Config,
    msg: &'a Message,
    backend_subject: &'a str,
    jsonrpc_method: &'a str,
    tenant: Option<String>,
    request_id: Option<serde_json::Value>,
    requires_spicedb: bool,
    spicedb_allowed: Option<bool>,
    traces: &'a TraceStore,
    audit_outcome: &'static str,
    jsonrpc_code: i32,
    jsonrpc_message: String,
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
    let envelope = AuditEnvelope {
        subject_in: params.msg.subject.to_string(),
        subject_out: params.backend_subject.to_string(),
        outcome: params.audit_outcome,
        direction: "request",
        jsonrpc_method: params.jsonrpc_method.to_string(),
        tenant: params.tenant,
        request_id: params.request_id.clone(),
    };
    let method_root = audit::jsonrpc_method_root(params.jsonrpc_method);
    let subject = audit::audit_publish_subject(prefix, params.audit_outcome, "request", &method_root);
    audit::publish_audit(
        params.jetstream,
        subject,
        &envelope,
        std::time::Duration::from_secs(5),
    )
    .await;

    if let Some(id) = params.request_id {
        params.traces.insert(
            id.to_string(),
            DecisionTrace {
                subject_in: params.msg.subject.to_string(),
                subject_out: params.backend_subject.to_string(),
                jsonrpc_method: params.jsonrpc_method.to_string(),
                cel_requires_spicedb: params.requires_spicedb,
                spicedb_allowed: params.spicedb_allowed,
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
    Some(headers?.get(TENANT_HEADER)?.as_str().to_string())
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
        assert_eq!(
            resources_read_uri(payload.as_bytes()).as_deref(),
            Some("file:///tmp/x")
        );
    }

    #[test]
    fn parse_server_id_ok() {
        assert_eq!(
            parse_server_id("mcp", "mcp.gateway.request.filesystem.tools.call").unwrap(),
            "filesystem"
        );
    }
}
