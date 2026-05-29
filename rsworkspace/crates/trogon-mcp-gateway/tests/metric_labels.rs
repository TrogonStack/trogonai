//! Integration scaffold for MCP gateway Prometheus metric labels and cardinality (Phase 1+).
//!
//! Scenario: Block G pins the gateway metric surface for request volume, latency histograms,
//! policy rule firing, SpiceDB checks, rate-limit drops, and audit publish outcomes. Tests
//! assert label keys, closed enum values, stable histogram buckets, and high-cardinality
//! safety (tool names and per-user identifiers stay off metric labels).
//!
//! Cross-references:
//! - `MCP_GATEWAY_PLAN.md` Block G operational section (metrics, `/metrics` export)
//! - `docs/identity/otel-wiring.md` (metric catalog, cardinality budget)
//! - OpenMetrics 1.0 text exposition (`Content-Type: application/openmetrics-text`)
//! - Prometheus naming and cardinality best practices
//!
//! Harness types: `mcp_nats::Config`, `McpPrefix`, `trogon_nats::NatsAuth`,
//! `trogon_mcp_gateway::gateway::GatewaySettings`, `trogon_mcp_gateway::trace::TraceStore`,
//! `trogon_mcp_gateway::audit::jsonrpc_method_root` (see `tests/e2e_nats_forward.rs`).
//!
//! Verification contract (implement when un-ignored):
//! - Scrape `GET /metrics` after driving ingress paths; parse OpenMetrics text.
//! - Assert series names, label sets, and bucket boundaries match the pinned surface below.

#![allow(unused_imports, dead_code)]

use std::sync::Arc;
use std::time::Duration;

use mcp_nats::{Config as McpConfig, McpPrefix};
use trogon_mcp_gateway::audit::jsonrpc_method_root;
use trogon_mcp_gateway::authz::AllowAllPermissionChecker;
use trogon_mcp_gateway::gateway::GatewaySettings;
use trogon_nats::{NatsAuth, NatsConfig};

/// Shared harness constants (see `tests/e2e_nats_forward.rs`, `tests/health_probes.rs`).
mod harness {
    use super::Duration;

    #[allow(dead_code)]
    pub const METRICS_LISTEN_ADDR: &str = "127.0.0.1:8080";
    #[allow(dead_code)]
    pub const METRICS_PATH: &str = "/metrics";
    #[allow(dead_code)]
    pub const OPENMETRICS_CONTENT_TYPE: &str = "application/openmetrics-text";
    #[allow(dead_code)]
    pub const NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
}

/// Pinned request counter: `mcp_requests_total{tenant, method_root, decision}`.
mod requests {
    use super::harness::{METRICS_LISTEN_ADDR, METRICS_PATH};
    use super::jsonrpc_method_root;

    pub const METRIC_NAME: &str = "mcp_requests_total";
    pub const LABEL_TENANT: &str = "tenant";
    pub const LABEL_METHOD_ROOT: &str = "method_root";
    pub const LABEL_DECISION: &str = "decision";

    #[allow(dead_code)]
    pub const DECISIONS: &[&str] = &["allow", "deny", "fault"];

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn mcp_requests_total_increments_on_allow_forward() {
        // Arrange: gateway allow path for `tools/list`; tenant from JWT/header
        // Act: scrape GET http://{METRICS_LISTEN_ADDR}{METRICS_PATH}
        // Assert: `mcp_requests_total{tenant="acme",method_root="tools",decision="allow"}` +1
        let _ = (METRICS_LISTEN_ADDR, METRICS_PATH);
        assert_eq!(jsonrpc_method_root("tools/list"), "tools");
        unimplemented!("mcp_requests_total allow series per Block G metric surface");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn mcp_requests_total_increments_on_policy_deny() {
        // Arrange: CEL deny before backend fan-out
        // Assert: decision label `deny` (not `error`)
        unimplemented!("mcp_requests_total deny series per Block G metric surface");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn mcp_requests_total_increments_on_fault() {
        // Arrange: bundle/CEL fault path (`-32101` class)
        // Assert: decision label `fault`
        unimplemented!("mcp_requests_total fault series per Block G metric surface");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn mcp_requests_total_rejects_unknown_decision_label() {
        // Arrange: in-process registry or scrape parse
        // Assert: only {allow, deny, fault} appear on `decision` label
        let _ = DECISIONS;
        unimplemented!("mcp_requests_total decision enum closed per Block G");
    }
}

/// Pinned latency histogram: `mcp_request_duration_seconds{tenant, method_root}`.
mod duration {
    use super::harness::{METRICS_LISTEN_ADDR, METRICS_PATH};
    use super::jsonrpc_method_root;

    pub const METRIC_NAME: &str = "mcp_request_duration_seconds";
    pub const LABEL_TENANT: &str = "tenant";
    pub const LABEL_METHOD_ROOT: &str = "method_root";

    /// Stable bucket boundaries (seconds) pinned for Phase 1+.
    pub const BUCKET_UPPER_BOUNDS: &[f64] = &[
        5e-4, 1e-3, 5e-3, 1e-2, 5e-2, 1e-1, 5e-1, 1.0, 5.0, 10.0,
    ];

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn mcp_request_duration_seconds_observes_allow_path() {
        // Arrange: completed allow forward for `tools/call`
        // Act: scrape histogram
        // Assert: `_sum` / `_count` move; labels tenant + method_root only
        let _ = (METRICS_LISTEN_ADDR, METRICS_PATH);
        assert_eq!(jsonrpc_method_root("tools/call"), "tools");
        unimplemented!("mcp_request_duration_seconds histogram per Block G");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn mcp_request_duration_seconds_bucket_boundaries_are_stable() {
        // Arrange: scrape after synthetic latency injection (or fixture clock)
        // Assert: `le` labels match BUCKET_UPPER_BOUNDS exactly (no drift across releases)
        let _ = BUCKET_UPPER_BOUNDS;
        unimplemented!("stable mcp_request_duration_seconds buckets per Block G");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn mcp_request_duration_seconds_uses_method_root_not_full_method() {
        // Arrange: ingress `resources/read`
        // Assert: label method_root="resources_read" (not `resources/read`)
        assert_eq!(jsonrpc_method_root("resources/read"), "resources_read");
        unimplemented!("method_root label on duration histogram per Block G");
    }
}

/// Pinned policy counter: `mcp_policy_rule_fired_total{rule_id}`.
mod policy {
    use super::harness::{METRICS_LISTEN_ADDR, METRICS_PATH};

    pub const METRIC_NAME: &str = "mcp_policy_rule_fired_total";
    pub const LABEL_RULE_ID: &str = "rule_id";

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn mcp_policy_rule_fired_total_increments_per_cel_rule_id() {
        // Arrange: bundle with two rules; traffic matches rule `deny-tools-call`
        // Act: scrape counter
        // Assert: `mcp_policy_rule_fired_total{rule_id="deny-tools-call"}` +1
        let _ = (METRICS_LISTEN_ADDR, METRICS_PATH);
        unimplemented!("mcp_policy_rule_fired_total per CEL rule id per Block G");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn mcp_policy_rule_fired_total_one_series_per_distinct_rule_id() {
        // Arrange: two rules fire on separate requests
        // Assert: two label values on rule_id, no aggregation across rules
        unimplemented!("one counter series per rule_id per Block G");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn mcp_policy_rule_fired_total_has_no_tenant_label() {
        // Arrange: multi-tenant traffic through same bundle revision
        // Assert: rule_id label only (tenant rollups use mcp_requests_total)
        unimplemented!("policy rule counter excludes tenant label per cardinality budget");
    }
}

/// Pinned SpiceDB counter: `mcp_spicedb_check_total{outcome, cache_hit}`.
mod spicedb {
    use super::harness::{METRICS_LISTEN_ADDR, METRICS_PATH};

    pub const METRIC_NAME: &str = "mcp_spicedb_check_total";
    pub const LABEL_OUTCOME: &str = "outcome";
    pub const LABEL_CACHE_HIT: &str = "cache_hit";

    #[allow(dead_code)]
    pub const OUTCOMES: &[&str] = &["permitted", "denied", "error"];

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn mcp_spicedb_check_total_permitted_series() {
        // Arrange: CheckBulkPermissions allow with fresh token
        // Assert: outcome="permitted", cache_hit="false"
        let _ = (METRICS_LISTEN_ADDR, METRICS_PATH);
        unimplemented!("mcp_spicedb_check_total permitted per Block G");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn mcp_spicedb_check_total_denied_series() {
        // Arrange: PDP deny
        // Assert: outcome="denied"
        unimplemented!("mcp_spicedb_check_total denied per Block G");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn mcp_spicedb_check_total_error_and_cache_hit_labels() {
        // Arrange: gRPC failure; separate request with ZedToken cache hit
        // Assert: outcome="error"; cache_hit="true" on cached path
        let _ = OUTCOMES;
        unimplemented!("mcp_spicedb_check_total outcome and cache_hit enums per Block G");
    }
}

/// Pinned rate-limit counter: `mcp_rate_limit_drops_total{scope}`.
mod rate_limit {
    use super::harness::{METRICS_LISTEN_ADDR, METRICS_PATH};

    pub const METRIC_NAME: &str = "mcp_rate_limit_drops_total";
    pub const LABEL_SCOPE: &str = "scope";

    #[allow(dead_code)]
    pub const SCOPES: &[&str] = &["tenant", "subject", "method_root", "global"];

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn mcp_rate_limit_drops_total_tenant_scope() {
        // Arrange: tenant bucket exhausted (`-32105`)
        // Assert: scope="tenant"
        let _ = (METRICS_LISTEN_ADDR, METRICS_PATH);
        unimplemented!("mcp_rate_limit_drops_total tenant scope per Block G");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn mcp_rate_limit_drops_total_subject_and_method_root_scopes() {
        // Arrange: per-subject and per-method_root caps
        // Assert: scope in {subject, method_root}
        unimplemented!("mcp_rate_limit_drops_total subject/method_root scopes per Block G");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn mcp_rate_limit_drops_total_global_scope() {
        // Arrange: fleet-wide ingress flood cap
        // Assert: scope="global"
        let _ = SCOPES;
        unimplemented!("mcp_rate_limit_drops_total global scope per Block G");
    }
}

/// Pinned audit counter: `mcp_audit_publish_total{outcome}`.
mod audit {
    use super::harness::{METRICS_LISTEN_ADDR, METRICS_PATH};

    pub const METRIC_NAME: &str = "mcp_audit_publish_total";
    pub const LABEL_OUTCOME: &str = "outcome";

    #[allow(dead_code)]
    pub const OUTCOMES: &[&str] = &["ok", "retry", "fail"];

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn mcp_audit_publish_total_ok_on_jetstream_ack() {
        // Arrange: audit publish succeeds
        // Assert: outcome="ok"
        let _ = (METRICS_LISTEN_ADDR, METRICS_PATH);
        unimplemented!("mcp_audit_publish_total ok per Block G");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn mcp_audit_publish_total_retry_on_transient_failure() {
        // Arrange: publish retried before ack
        // Assert: outcome="retry"
        unimplemented!("mcp_audit_publish_total retry per Block G");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn mcp_audit_publish_total_fail_after_exhausted_retries() {
        // Arrange: JetStream publish fails permanently (request path still completes)
        // Assert: outcome="fail"
        let _ = OUTCOMES;
        unimplemented!("mcp_audit_publish_total fail per Block G");
    }
}

/// High-cardinality safety: forbidden metric labels vs span attributes.
mod cardinality {
    use super::harness::{METRICS_LISTEN_ADDR, METRICS_PATH};

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn tool_name_is_not_a_prometheus_label() {
        // Arrange: `tools/call` with params.name="create_issue"
        // Act: scrape all `mcp_*` series
        // Assert: no label key `tool` or `tool_name`; span attribute `mcp.tool` present on trace
        let _ = (METRICS_LISTEN_ADDR, METRICS_PATH);
        unimplemented!("tool name on span mcp.tool only, not metric labels per Block G");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn per_user_labels_are_not_exposed_on_metrics() {
        // Arrange: distinct JWT subjects under same tenant
        // Assert: no `user`, `user_id`, `subject`, `agent_id`, or `session_id` label keys
        unimplemented!("per-user identifiers excluded from metric labels per cardinality budget");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn request_id_and_trace_id_are_not_metric_labels() {
        // Arrange: high-volume ingress with unique JSON-RPC ids
        // Assert: scraped series label key set is subset of pinned v1 keys only
        unimplemented!("request_id and trace_id never metric labels per otel-wiring.md §8");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn nats_subject_strings_are_not_metric_labels() {
        // Arrange: varying ingress subject suffixes
        // Assert: no `subject` or `subject_in` label on gateway counters/histograms
        unimplemented!("NATS subject strings span-only per Prometheus cardinality best practices");
    }
}

/// `/metrics` exposition format (OpenMetrics text).
mod format {
    use super::harness::{METRICS_LISTEN_ADDR, METRICS_PATH, OPENMETRICS_CONTENT_TYPE};

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn metrics_endpoint_returns_openmetrics_content_type() {
        // Arrange: gateway operational listener up
        // Act: GET /metrics
        // Assert: Content-Type == application/openmetrics-text
        let _ = (METRICS_LISTEN_ADDR, METRICS_PATH, OPENMETRICS_CONTENT_TYPE);
        unimplemented!("OpenMetrics Content-Type per Block G /metrics endpoint");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn metrics_body_is_openmetrics_text_exposition() {
        // Arrange: scrape after at least one allow request
        // Assert: HELP/TYPE lines; counter/histogram syntax; `# EOF` trailer if required
        unimplemented!("OpenMetrics text body per OpenMetrics 1.0 spec");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when metric surface per MCP_GATEWAY_PLAN.md Block G lands"]
    async fn metrics_endpoint_exposes_pinned_mcp_series_names() {
        // Arrange: drive allow, deny, rate-limit, and audit paths
        // Assert: body contains mcp_requests_total, mcp_request_duration_seconds,
        //         mcp_policy_rule_fired_total, mcp_spicedb_check_total,
        //         mcp_rate_limit_drops_total, mcp_audit_publish_total
        unimplemented!("pinned metric family names on /metrics per Block G");
    }
}
