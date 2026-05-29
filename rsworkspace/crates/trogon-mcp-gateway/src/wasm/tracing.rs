//! W3C trace propagation and guest observability on the WASM evaluate span (ADR 0032).

use async_nats::header::HeaderMap;
use tracing::Span;
use trogon_nats::inject_trace_context;

use super::bindings::{HostFailure, LogLevel, RequestCtx, SpanContext};

/// OpenTelemetry span name for one Tier-3 `evaluate()` invocation.
pub const WASM_EVALUATE_SPAN_NAME: &str = "mcp.gateway.wasm.evaluate";

const MAX_ATTR_KEY_BYTES: usize = 64;
const MAX_ATTR_VALUE_BYTES: usize = 256;
const MAX_EVENT_NAME_BYTES: usize = 64;

/// Parsed W3C `traceparent` (`version-trace_id-parent_id-flags`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTraceParent {
    pub version: String,
    pub trace_id: String,
    pub parent_span_id: String,
    pub flags: String,
}

/// Builds a [`SpanContext`] snapshot from the active `tracing` span (W3C inject).
#[must_use]
pub fn span_context_from_current() -> SpanContext {
    let mut headers = HeaderMap::new();
    inject_trace_context(&mut headers);
    let traceparent = extract_traceparent_from_headers(&headers).unwrap_or_default();
    let tracestate = headers
        .get("tracestate")
        .map(|value| value.as_str().to_string());
    let trace_id = extract_trace_id(&traceparent).unwrap_or_else(mint_trace_id);
    SpanContext {
        trace_id,
        traceparent,
        tracestate,
    }
}

/// Fills `request.span` from [`Span::current()`] before guest `evaluate` (Block F item 3).
pub fn populate_request_span(request: &mut RequestCtx) {
    let injected = span_context_from_current();
    if !injected.traceparent.is_empty() {
        request.span.traceparent = injected.traceparent;
        request.span.trace_id = injected.trace_id;
        request.span.tracestate = injected.tracestate;
    } else if request.span.trace_id.is_empty() {
        request.span.trace_id = extract_trace_id(&request.span.traceparent)
            .unwrap_or_else(mint_trace_id);
    }
}

/// Returns the W3C `traceparent` carrier on a request snapshot.
#[must_use]
pub fn extract_traceparent(ctx: &SpanContext) -> &str {
    &ctx.traceparent
}

/// Parses `span.traceparent` for parent linkage tests and ingress correlation.
#[must_use]
pub fn parent_from_span_context(ctx: &SpanContext) -> Option<ParsedTraceParent> {
    parse_traceparent(&ctx.traceparent)
}

/// 32-hex trace id from a W3C `traceparent` value.
#[must_use]
pub fn extract_trace_id(traceparent: &str) -> Option<String> {
    let parsed = parse_traceparent(traceparent)?;
    Some(parsed.trace_id)
}

/// Whether the W3C sampled flag (low bit) is set on `traceparent`.
#[must_use]
pub fn traceparent_is_sampled(traceparent: &str) -> bool {
    parse_traceparent(traceparent)
        .and_then(|parsed| parsed.flags.parse::<u8>().ok())
        .is_some_and(|flags| flags & 0x01 == 0x01)
}

/// Maps WIT `log-level` to `tracing::Level`.
#[must_use]
pub fn tracing_level_from_log(level: LogLevel) -> tracing::Level {
    match level {
        LogLevel::Trace => tracing::Level::TRACE,
        LogLevel::Debug => tracing::Level::DEBUG,
        LogLevel::Info => tracing::Level::INFO,
        LogLevel::Warn => tracing::Level::WARN,
        LogLevel::Error => tracing::Level::ERROR,
    }
}

/// Records a guest attribute on the active evaluate span.
pub fn set_guest_span_attribute(key: &str, value: &str) -> Result<(), HostFailure> {
    validate_guest_attribute(key, value)?;
    Span::current().record(key, value);
    Ok(())
}

/// Records a guest event on the active evaluate span.
pub fn set_guest_span_event(name: &str, attributes_json: &str) -> Result<(), HostFailure> {
    validate_guest_event_name(name)?;
    if attributes_json.len() > 4096 {
        return Err(HostFailure {
            code: "span_event_rejected".into(),
            message: "attributes-json exceeds size cap".into(),
        });
    }
    let _ = serde_json::from_str::<serde_json::Value>(attributes_json).map_err(|err| HostFailure {
        code: "span_event_rejected".into(),
        message: err.to_string(),
    })?;
    tracing::info!(
        parent: &Span::current(),
        otel.name = name,
        wasm.event.attributes_json = attributes_json,
        "wasm span event"
    );
    Ok(())
}

fn extract_traceparent_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get("traceparent")
        .map(|value| value.as_str().to_string())
}

fn parse_traceparent(traceparent: &str) -> Option<ParsedTraceParent> {
    let mut parts = traceparent.split('-');
    let version = parts.next()?.to_string();
    let trace_id = parts.next()?;
    let parent_span_id = parts.next()?;
    let flags = parts.next()?;
    if trace_id.len() != 32 || parent_span_id.len() != 16 {
        return None;
    }
    Some(ParsedTraceParent {
        version,
        trace_id: trace_id.to_string(),
        parent_span_id: parent_span_id.to_string(),
        flags: flags.to_string(),
    })
}

fn mint_trace_id() -> String {
    format!(
        "{:032x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u128
    )
}

fn validate_guest_attribute(key: &str, value: &str) -> Result<(), HostFailure> {
    if !key.starts_with("wasm.") {
        return Err(HostFailure {
            code: "span_attribute_rejected".into(),
            message: "attribute key must start with wasm.".into(),
        });
    }
    if key.len() > MAX_ATTR_KEY_BYTES {
        return Err(HostFailure {
            code: "span_attribute_rejected".into(),
            message: "attribute key exceeds 64 bytes".into(),
        });
    }
    if value.len() > MAX_ATTR_VALUE_BYTES {
        return Err(HostFailure {
            code: "span_attribute_rejected".into(),
            message: "attribute value exceeds 256 bytes".into(),
        });
    }
    Ok(())
}

fn validate_guest_event_name(name: &str) -> Result<(), HostFailure> {
    if !name.starts_with("wasm.") {
        return Err(HostFailure {
            code: "span_event_rejected".into(),
            message: "event name must start with wasm.".into(),
        });
    }
    if name.len() > MAX_EVENT_NAME_BYTES {
        return Err(HostFailure {
            code: "span_event_rejected".into(),
            message: "event name exceeds 64 bytes".into(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wasm::bindings::RequestCtx;

    fn sample_traceparent() -> &'static str {
        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
    }

    #[test]
    fn extract_trace_id_from_traceparent() {
        assert_eq!(
            extract_trace_id(sample_traceparent()).as_deref(),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
    }

    #[test]
    fn parent_from_span_context_round_trips_request_ctx() {
        let ctx = SpanContext {
            trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".into(),
            traceparent: sample_traceparent().into(),
            tracestate: Some("vendor=value".into()),
        };
        let parent = parent_from_span_context(&ctx).expect("parsed");
        assert_eq!(parent.trace_id, ctx.trace_id);
        assert_eq!(extract_traceparent(&ctx), sample_traceparent());
        assert_eq!(parent.flags, "01");
    }

    #[test]
    fn traceparent_sampling_flag_parsed() {
        assert!(traceparent_is_sampled(sample_traceparent()));
        assert!(!traceparent_is_sampled(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00"
        ));
    }

    #[test]
    fn populate_request_span_fills_trace_id_from_traceparent() {
        let mut request = RequestCtx {
            request_id: "r1".into(),
            actor_id: "user:alice".into(),
            subject_id: "user:alice".into(),
            method: "tools/call".into(),
            params_json: "{}".into(),
            act_chain_json: "[]".into(),
            attributes_json: "{}".into(),
            span: SpanContext {
                trace_id: String::new(),
                traceparent: sample_traceparent().into(),
                tracestate: None,
            },
            tools: vec![],
        };
        populate_request_span(&mut request);
        assert_eq!(request.span.trace_id, "4bf92f3577b34da6a3ce929d0e0e4736");
    }

    #[test]
    fn span_attribute_set_rejects_non_wasm_prefix() {
        let err = set_guest_span_attribute("policy.tier", "wasm")
            .expect_err("rejected");
        assert_eq!(err.code, "span_attribute_rejected");
    }

    #[test]
    fn tracing_level_from_log_maps_variants() {
        assert_eq!(tracing_level_from_log(LogLevel::Warn), tracing::Level::WARN);
        assert_eq!(tracing_level_from_log(LogLevel::Error), tracing::Level::ERROR);
    }
}
