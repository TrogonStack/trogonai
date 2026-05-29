//! OTLP trace export for gateway `tracing` spans via `tracing-opentelemetry`.
//!
//! # Span name mapping (ADR 0031 / otel-wiring.md → emitted today)
//!
//! | Normative (otel-wiring.md §3) | Emitted span name in this crate |
//! |---|---|
//! | `mcp.gateway.request` | `mcp_gateway.handle_ingress` (`gateway.rs`) |
//! | `mcp.gateway.authz` | *(not emitted yet — future `policy.rs` hook)* |
//! | `mcp.gateway.spicedb.check` | *(not emitted yet — future `spicedb.rs` hook)* |
//! | `mcp.gateway.egress` | *(not emitted yet — future egress hook)* |
//! | `mcp.gateway.audit.publish` | *(not emitted yet — future `audit.rs` hook)* |
//! | `mcp.gateway.wasm.evaluate` | `mcp.gateway.wasm.evaluate` (`wasm/engine.rs`) |
//! | `mcp.gateway.plugin.call` | `mcp.gateway.plugin.call` (`plugin/dispatcher.rs`) |
//! | `nats.request` | `nats.request` (`trogon-nats`, child under egress when wired) |
//!
//! Export uses `opentelemetry-otlp` through `trogon-telemetry` (workspace dep graph).
//! `init_otel_exporter` applies `TROGON_OTEL_*` env overrides then delegates OTLP/HTTP
//! batch export to `trogon_telemetry::init_logger`.

use trogon_std::env::SystemEnv;
use trogon_std::fs::SystemFs;
use trogon_telemetry::{ResourceAttribute, ServiceName};

use super::config::ObservabilityConfig;
use super::errors::ObservabilityError;

/// Span names the gateway emits today (used by conformance tests).
pub const EMITTED_GATEWAY_SPAN_NAMES: &[&str] = &[
    "mcp_gateway.handle_ingress",
    "mcp.gateway.wasm.evaluate",
    "mcp.gateway.plugin.call",
];

/// Normative Block G span catalog (otel-wiring.md §3) for forward-compat checks.
#[allow(dead_code)]
pub const NORMATIVE_GATEWAY_SPAN_NAMES: &[&str] = &[
    "mcp.gateway.request",
    "mcp.gateway.authz",
    "mcp.gateway.spicedb.check",
    "mcp.gateway.egress",
    "mcp.gateway.audit.publish",
    "mcp.gateway.wasm.evaluate",
    "mcp.gateway.plugin.call",
];

/// Returns the allowlist of span names currently emitted by the gateway binary.
#[must_use]
pub fn gateway_span_allowlist() -> &'static [&'static str] {
    EMITTED_GATEWAY_SPAN_NAMES
}

/// RAII guard: flushes and shuts down OTel providers on drop.
pub struct OtelGuard {
    active: bool,
}

impl OtelGuard {
    #[must_use]
    fn noop() -> Self {
        Self { active: false }
    }

    #[must_use]
    fn active() -> Self {
        Self { active: true }
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }
}

impl Drop for OtelGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = trogon_telemetry::shutdown_otel();
        }
    }
}

/// Applies operator config, wires `tracing-opentelemetry` via `trogon-telemetry`, and returns
/// a guard that shuts down exporters on drop.
///
/// When `otel_endpoint` is unset, returns a no-op guard so callers can always invoke this at
/// startup. Idempotent with respect to an already-initialized global subscriber (logs a warning
/// and still returns an active guard when export was configured).
pub fn init_otel_exporter(config: ObservabilityConfig) -> Result<OtelGuard, ObservabilityError> {
    if config.otel_endpoint.is_none() {
        return Ok(OtelGuard::noop());
    }

    config.apply_otel_env()?;

    trogon_telemetry::init_logger(
        ServiceName::TrogonMcpGateway,
        [ResourceAttribute::mcp_prefix(&config.mcp_prefix)],
        &SystemEnv,
        &SystemFs,
    );

    Ok(OtelGuard::active())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wasm::WASM_EVALUATE_SPAN_NAME;

    #[test]
    fn emitted_allowlist_includes_wasm_evaluate_and_handle_ingress() {
        let allowlist = gateway_span_allowlist();
        assert!(allowlist.contains(&"mcp_gateway.handle_ingress"));
        assert!(allowlist.contains(&WASM_EVALUATE_SPAN_NAME));
        assert!(allowlist.contains(&"mcp.gateway.plugin.call"));
    }

    #[test]
    fn normative_catalog_is_superset_of_emitted_names_modulo_mapping() {
        assert!(NORMATIVE_GATEWAY_SPAN_NAMES.contains(&"mcp.gateway.wasm.evaluate"));
        assert!(NORMATIVE_GATEWAY_SPAN_NAMES.contains(&"mcp.gateway.plugin.call"));
    }

    #[test]
    fn init_without_endpoint_returns_noop_guard() {
        let config = ObservabilityConfig {
            otel_endpoint: None,
            otel_service_name: None,
            siem_subject: None,
            audit_consumer_durable: super::super::config::DEFAULT_AUDIT_CONSUMER.into(),
            siem_format: super::super::audit_bridge::SiemFormat::Raw,
            audit_stream_name: super::super::config::DEFAULT_AUDIT_STREAM.into(),
            mcp_prefix: super::super::config::DEFAULT_MCP_PREFIX.into(),
        };
        let guard = init_otel_exporter(config).expect("noop init");
        assert!(!guard.is_active());
    }
}
