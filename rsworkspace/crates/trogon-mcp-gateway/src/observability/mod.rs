//! OpenTelemetry export and audit→SIEM bridge (Block G item 6).

mod audit_bridge;
mod config;
mod errors;
mod otel;

pub use audit_bridge::{
    reshape_audit_message, AuditBridge, ReshapedAuditEvent, SiemFormat, SPAN_ID_HEADER,
    TRACE_ID_HEADER, TRACEPARENT_HEADER,
};
pub use config::ObservabilityConfig;
pub use errors::ObservabilityError;
pub use otel::{gateway_span_allowlist, init_otel_exporter, OtelGuard};

pub use config::{
    DEFAULT_AUDIT_CONSUMER, DEFAULT_AUDIT_STREAM, DEFAULT_MCP_PREFIX, ENV_AUDIT_CONSUMER,
    ENV_AUDIT_STREAM, ENV_MCP_PREFIX, ENV_OTEL_ENDPOINT, ENV_OTEL_SERVICE_NAME, ENV_SIEM_FORMAT,
    ENV_SIEM_SUBJECT,
};
