//! JetStream consumer: audit stream → SIEM subject (or stdout dry-run).

use std::io::{self, Write};

use async_nats::header::HeaderMap;
use async_nats::jetstream::consumer::pull;
use async_nats::jetstream::{self};
use bytes::Bytes;
use futures::StreamExt;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::audit::ensure_audit_stream;

use super::config::ObservabilityConfig;
use super::errors::ObservabilityError;

pub const TRACE_ID_HEADER: &str = "trace_id";
pub const SPAN_ID_HEADER: &str = "span_id";
pub const TRACEPARENT_HEADER: &str = "traceparent";

/// SIEM destination encoding (Pin 7 envelope is SIEM-compatible in `raw` mode).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SiemFormat {
    Raw,
    SplunkHec,
    ElasticEcs,
}

impl SiemFormat {
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "splunk-hec" | "splunk_hec" | "splunk" => Self::SplunkHec,
            "elastic-ecs" | "elastic_ecs" | "ecs" => Self::ElasticEcs,
            _ => Self::Raw,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::SplunkHec => "splunk-hec",
            Self::ElasticEcs => "elastic-ecs",
        }
    }
}

/// Outbound SIEM payload plus optional correlation headers (Pin 7 `trace_id` / `span_id`).
#[derive(Debug, PartialEq, Eq)]
pub struct ReshapedAuditEvent {
    pub body: Bytes,
    pub headers: HeaderMap,
}

/// Re-shape an audit JetStream message for the configured SIEM format.
pub fn reshape_audit_message(
    audit_subject: &str,
    payload: &[u8],
    inbound_headers: Option<&HeaderMap>,
    format: SiemFormat,
) -> Result<ReshapedAuditEvent, ObservabilityError> {
    let trace_context = extract_trace_context(inbound_headers);
    match format {
        SiemFormat::Raw => reshape_raw(payload, &trace_context),
        SiemFormat::SplunkHec => reshape_splunk_hec(audit_subject, payload, &trace_context),
        SiemFormat::ElasticEcs => reshape_elastic_ecs(audit_subject, payload, &trace_context),
    }
}

fn reshape_raw(payload: &[u8], trace_context: &TraceContext) -> Result<ReshapedAuditEvent, ObservabilityError> {
    serde_json::from_slice::<serde_json::Value>(payload).map_err(|error| {
        ObservabilityError::Reshape(format!("audit payload is not valid JSON: {error}"))
    })?;

    let mut headers = HeaderMap::new();
    apply_trace_headers(&mut headers, trace_context);

    Ok(ReshapedAuditEvent {
        body: Bytes::copy_from_slice(payload),
        headers,
    })
}

/// Splunk HEC JSON envelope — transform belongs in `reshape_splunk_hec` once HEC auth/index
/// metadata is operator-configurable.
fn reshape_splunk_hec(
    _audit_subject: &str,
    _payload: &[u8],
    _trace_context: &TraceContext,
) -> Result<ReshapedAuditEvent, ObservabilityError> {
    Err(ObservabilityError::Reshape(
        "siem_format=splunk-hec is not implemented; add HEC envelope in reshape_splunk_hec"
            .into(),
    ))
}

/// Elastic ECS document — transform belongs in `reshape_elastic_ecs` (see trogon-traffic-view OCSF
/// exporter for field mapping reference).
fn reshape_elastic_ecs(
    _audit_subject: &str,
    _payload: &[u8],
    _trace_context: &TraceContext,
) -> Result<ReshapedAuditEvent, ObservabilityError> {
    Err(ObservabilityError::Reshape(
        "siem_format=elastic-ecs is not implemented; add ECS mapping in reshape_elastic_ecs"
            .into(),
    ))
}

#[derive(Debug, Default, PartialEq, Eq)]
struct TraceContext {
    trace_id: Option<String>,
    span_id: Option<String>,
    traceparent: Option<String>,
}

fn extract_trace_context(headers: Option<&HeaderMap>) -> TraceContext {
    let Some(headers) = headers else {
        return TraceContext::default();
    };

    let traceparent = headers
        .get(TRACEPARENT_HEADER)
        .map(|value| value.as_str().to_string());

    let mut trace_id = headers
        .get(TRACE_ID_HEADER)
        .map(|value| value.as_str().to_string());
    let mut span_id = headers
        .get(SPAN_ID_HEADER)
        .map(|value| value.as_str().to_string());

    if let Some(parent) = traceparent.as_deref() {
        if let Some(parsed) = parse_traceparent(parent) {
            trace_id.get_or_insert(parsed.trace_id);
            span_id.get_or_insert(parsed.span_id);
        }
    }

    TraceContext {
        trace_id,
        span_id,
        traceparent,
    }
}

fn parse_traceparent(value: &str) -> Option<ParsedTraceParent> {
    let mut parts = value.split('-');
    let version = parts.next()?;
    if version != "00" {
        return None;
    }
    let trace_id = parts.next()?.to_string();
    let span_id = parts.next()?.to_string();
    let _flags = parts.next()?;
    if trace_id.len() != 32 || span_id.len() != 16 {
        return None;
    }
    Some(ParsedTraceParent { trace_id, span_id })
}

struct ParsedTraceParent {
    trace_id: String,
    span_id: String,
}

fn apply_trace_headers(headers: &mut HeaderMap, trace: &TraceContext) {
    if let Some(trace_id) = &trace.trace_id {
        headers.insert(TRACE_ID_HEADER, trace_id.as_str());
    }
    if let Some(span_id) = &trace.span_id {
        headers.insert(SPAN_ID_HEADER, span_id.as_str());
    }
    if let Some(traceparent) = &trace.traceparent {
        headers.insert(TRACEPARENT_HEADER, traceparent.as_str());
    }
}

/// JetStream → SIEM bridge worker.
pub struct AuditBridge;

impl AuditBridge {
    /// Starts a background task that consumes `{prefix}.audit.>` and republishes to `siem_subject`.
    ///
    /// Pass `shutdown` to stop the loop gracefully (`select!` against the consumer stream).
    pub fn start(
        nats: async_nats::Client,
        config: ObservabilityConfig,
        shutdown: oneshot::Receiver<()>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            if let Err(error) = run_bridge(nats, config, shutdown).await {
                warn!(%error, "audit→SIEM bridge stopped with error");
            }
        })
    }
}

async fn run_bridge(
    nats: async_nats::Client,
    config: ObservabilityConfig,
    mut shutdown: oneshot::Receiver<()>,
) -> Result<(), ObservabilityError> {
    let jetstream = jetstream::new(nats.clone());
    ensure_audit_stream(
        &jetstream,
        &config.audit_stream_name,
        &config.mcp_prefix,
    )
    .await
    .map_err(|error| ObservabilityError::AuditBridge(error.to_string()))?;

    let stream = jetstream
        .get_stream(&config.audit_stream_name)
        .await
        .map_err(|error| ObservabilityError::AuditBridge(error.to_string()))?;

    let filter = config.audit_filter_subject();
    let consumer = stream
        .create_consumer(pull::Config {
            durable_name: Some(config.audit_consumer_durable.clone()),
            filter_subject: filter.clone(),
            ack_policy: jetstream::consumer::AckPolicy::Explicit,
            ..Default::default()
        })
        .await
        .map_err(|error| ObservabilityError::AuditBridge(error.to_string()))?;

    let mut messages = consumer
        .messages()
        .await
        .map_err(|error| ObservabilityError::AuditBridge(error.to_string()))?;

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                debug!("audit→SIEM bridge shutdown signal received");
                break;
            }
            next = messages.next() => {
                let Some(result) = next else {
                    break;
                };
                let message = result.map_err(|error| ObservabilityError::AuditBridge(error.to_string()))?;
                let subject = message.subject.to_string();
                if let Err(error) = forward_message(&nats, &config, &subject, &message.payload, message.headers.as_ref()).await {
                    warn!(%subject, %error, "audit→SIEM reshape/publish failed");
                    continue;
                }
                if let Err(error) = message.ack().await {
                    warn!(%subject, %error, "audit→SIEM ack failed");
                }
            }
        }
    }

    Ok(())
}

async fn forward_message(
    nats: &async_nats::Client,
    config: &ObservabilityConfig,
    audit_subject: &str,
    payload: &[u8],
    inbound_headers: Option<&HeaderMap>,
) -> Result<(), ObservabilityError> {
    let reshaped = reshape_audit_message(audit_subject, payload, inbound_headers, config.siem_format)?;

    if config.siem_dry_run_stdout() {
        let line = String::from_utf8_lossy(&reshaped.body);
        let mut stdout = io::stdout().lock();
        writeln!(stdout, "{audit_subject}\t{line}").map_err(|error| {
            ObservabilityError::AuditBridge(format!("stdout dry-run write failed: {error}"))
        })?;
        stdout.flush().ok();
        return Ok(());
    }

    let Some(siem_subject) = config.siem_subject.as_deref() else {
        return Err(ObservabilityError::AuditBridge(
            "TROGON_SIEM_SUBJECT is unset; set subject or use '-' for stdout dry-run".into(),
        ));
    };

    nats.publish_with_headers(siem_subject.to_string(), reshaped.headers, reshaped.body)
        .await
        .map_err(|error| ObservabilityError::AuditBridge(error.to_string()))?;
    nats.flush().await.map_err(|error| ObservabilityError::AuditBridge(error.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_payload() -> Vec<u8> {
        br#"{
            "subject_in": "mcp.gateway.request.fs.tools.call",
            "subject_out": "mcp.server.fs.tools.call",
            "outcome": "allow",
            "direction": "request",
            "jsonrpc_method": "tools/call",
            "tenant": "acme",
            "identity_source": "jwt"
        }"#
        .to_vec()
    }

    #[test]
    fn raw_round_trip_preserves_payload_bytes() {
        let payload = sample_payload();
        let mut headers = HeaderMap::new();
        headers.insert(
            TRACEPARENT_HEADER,
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
        );

        let reshaped = reshape_audit_message(
            "mcp.audit.allow.request.tools",
            &payload,
            Some(&headers),
            SiemFormat::Raw,
        )
        .expect("reshape");

        assert_eq!(reshaped.body.as_ref(), payload.as_slice());
        assert_eq!(
            reshaped.headers.get(TRACE_ID_HEADER).map(|v| v.as_str()),
            Some("0af7651916cd43dd8448eb211c80319c")
        );
        assert_eq!(
            reshaped.headers.get(SPAN_ID_HEADER).map(|v| v.as_str()),
            Some("b7ad6b7169203331")
        );
    }

    #[test]
    fn splunk_and_ecs_formats_are_stubbed() {
        let payload = sample_payload();
        assert!(reshape_audit_message("mcp.audit.allow.request.tools", &payload, None, SiemFormat::SplunkHec).is_err());
        assert!(reshape_audit_message("mcp.audit.allow.request.tools", &payload, None, SiemFormat::ElasticEcs).is_err());
    }

    #[test]
    fn siem_format_parse_is_case_insensitive() {
        assert_eq!(SiemFormat::parse("RAW"), SiemFormat::Raw);
        assert_eq!(SiemFormat::parse("splunk-hec"), SiemFormat::SplunkHec);
        assert_eq!(SiemFormat::parse("elastic-ecs"), SiemFormat::ElasticEcs);
    }
}
