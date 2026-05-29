//! Integration scaffold for MCP gateway OpenTelemetry span shape (Wire-Format Pin 7).
//!
//! Scenario: Phase 1+ pins one root span per ingress request with required attributes,
//! optional method-specific fields, child spans for SpiceDB, span events for CEL rule
//! firing, ERROR status on JSON-RPC failures, and OTLP export for operator correlation.
//!
//! Cross-references:
//! - `docs/identity/reference-audit-envelope.md` (`latency_us`, routing fields, `spicedb` block)
//! - `docs/identity/otel-wiring.md` (span hierarchy, propagation, redaction rules)
//! - `reference-audit-envelope.md` (audit envelope) and Block G OTel export
//! - OpenTelemetry semantic conventions for messaging and RPC spans
//!
//! Harness types: `mcp_nats::Config`, `McpPrefix`, `trogon_nats::NatsAuth`,
//! `trogon_mcp_gateway::gateway::GatewaySettings`, `trogon_mcp_gateway::trace::TraceStore`
//! (see `e2e_nats_forward.rs`). Span capture via in-memory OTLP exporter when wired.
//!
//! Once OTel span shape lands: remove `#[ignore]`, drive the NATS gateway path, and assert
//! exported spans match the pinned name, attribute set, events, status, and latency budget.

#![allow(unused_imports)]

use std::sync::Arc;
use std::time::Duration;

use mcp_nats::{Config as McpConfig, McpPrefix};
use trogon_mcp_gateway::audit::jsonrpc_method_root;
use trogon_mcp_gateway::authz::AllowAllPermissionChecker;
use trogon_mcp_gateway::gateway::GatewaySettings;
use trogon_nats::{NatsAuth, NatsConfig};

mod root_span {
    //! Root span name is `mcp.<method_root>` derived from JSON-RPC method (not full method string).

    use super::jsonrpc_method_root;

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn tools_call_root_span_name_is_mcp_tools_call() {
        // Arrange: gateway handles ingress `tools/call` on `{prefix}.gateway.request.*.tools.call`
        // Act: capture exported root span after allow forward completes
        // Assert: span.name == "mcp.tools/call"
        assert_eq!(jsonrpc_method_root("tools/call"), "tools");
        unimplemented!("root span name mcp.tools/call per Wire-Format Pin 7 OTel shape");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn tools_list_root_span_name_is_mcp_tools_list() {
        // Arrange: ingress `tools/list`
        // Assert: span.name == "mcp.tools/list"
        assert_eq!(jsonrpc_method_root("tools/list"), "tools");
        unimplemented!("root span name mcp.tools/list per Wire-Format Pin 7 OTel shape");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn initialize_root_span_name_is_mcp_initialize() {
        // Arrange: ingress `initialize`
        // Assert: span.name == "mcp.initialize"
        assert_eq!(jsonrpc_method_root("initialize"), "initialize");
        unimplemented!("root span name mcp.initialize per Wire-Format Pin 7 OTel shape");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn resources_read_root_span_name_uses_underscore_method_root() {
        // Arrange: ingress `resources/read`
        // Assert: span.name == "mcp.resources/read" (method_root segment `resources_read`)
        assert_eq!(jsonrpc_method_root("resources/read"), "resources_read");
        unimplemented!("root span name mcp.resources/read per Wire-Format Pin 7 OTel shape");
    }
}

mod attributes {
    //! Required attributes on every request span regardless of outcome.

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn every_span_includes_mcp_method_and_method_root() {
        // Arrange: any forwarded MCP method through gateway
        // Assert: attributes `mcp.method` (full JSON-RPC method) and `mcp.method_root`
        //         (first segment, dots as underscores) are set on root span
        unimplemented!("required mcp.method and mcp.method_root attributes per Pin 7");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn every_span_includes_mcp_server_id_and_tenant() {
        // Arrange: ingress with `mcp-tenant` header / JWT tenant claim
        // Assert: `mcp.server_id` from subject, `tenant` from identity layer
        unimplemented!("required mcp.server_id and tenant attributes per Pin 7");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn every_span_includes_subject_in_subject_out_direction_decision() {
        // Arrange: allow path with known ingress/egress subjects
        // Assert: `subject_in`, `subject_out`, `direction`, `decision` mirror audit envelope
        unimplemented!("routing attributes subject_in/out direction decision per audit envelope");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn required_attributes_present_on_deny_without_egress() {
        // Arrange: policy deny before backend fan-out
        // Assert: same required attribute set on root span; `decision=deny`
        unimplemented!("required attributes on deny path without subject_out forward");
    }
}

mod tools_call {
    //! `tools/call` adds `mcp.tool`; other methods must not set it.

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn tools_call_span_includes_mcp_tool_attribute() {
        // Arrange: `tools/call` with `params.name = "create_issue"`
        // Assert: root span attribute `mcp.tool` == tool name string
        unimplemented!("mcp.tool attribute on tools/call per Pin 7 OTel shape");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn tools_list_span_omits_mcp_tool_attribute() {
        // Arrange: `tools/list` allow path
        // Assert: exported root span has no `mcp.tool` key
        unimplemented!("tools/list must omit mcp.tool attribute");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn initialize_span_omits_mcp_tool_attribute() {
        // Arrange: `initialize` handshake
        // Assert: no `mcp.tool` on root span
        unimplemented!("initialize must omit mcp.tool attribute");
    }
}

mod latency {
    //! Root span duration correlates with audit envelope `latency_us`.

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn span_duration_matches_audit_envelope_latency_us_within_tolerance() {
        // Arrange: capture root span end time and JetStream audit envelope for same request
        // Assert: span duration (microseconds) within tolerance of envelope `latency_us`
        unimplemented!("span duration vs audit latency_us correlation per reference-audit-envelope.md");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn span_duration_includes_authz_and_egress_on_allow_path() {
        // Arrange: gated `tools/call` with SpiceDB consult and backend reply
        // Assert: root span covers full gateway path; child spans nest under root
        unimplemented!("root span wall time covers authz egress on allow path");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn early_deny_span_duration_matches_audit_latency_us() {
        // Arrange: CEL deny before egress
        // Assert: shorter root span still matches audit `latency_us` field
        unimplemented!("deny-path latency_us matches root span duration");
    }
}

mod spicedb_child {
    //! SpiceDB consultation emits child span `spicedb.check` with token and cache attributes.

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn spicedb_consult_creates_child_span_spicedb_check() {
        // Arrange: `tools/call` on gated method with SpiceDB endpoint configured
        // Assert: child span name `spicedb.check` parented to root request span
        unimplemented!("child span spicedb.check per otel-wiring.md and Pin 7");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn spicedb_child_includes_zedtoken_and_cache_hit_attributes() {
        // Arrange: SpiceDB check with warm ZedToken cache
        // Assert: `spicedb.zedtoken`, `spicedb.cache_hit` on child span
        unimplemented!("spicedb.zedtoken and spicedb.cache_hit on spicedb.check child");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn non_gated_method_omits_spicedb_check_child_span() {
        // Arrange: `tools/list` without SpiceDB gate
        // Assert: no `spicedb.check` child under root span
        unimplemented!("omit spicedb.check child when SpiceDB not consulted");
    }
}

mod policy_event {
    //! CEL rule firing records span event `policy.rule_fired`.

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn cel_rule_firing_adds_policy_rule_fired_event() {
        // Arrange: policy bundle with named rule that matches request
        // Assert: root or authz span event name `policy.rule_fired` with rule id attribute
        unimplemented!("span event policy.rule_fired when CEL rule matches");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn multiple_rules_fired_emit_distinct_policy_rule_fired_events() {
        // Arrange: merged policy with two matching rules (see audit `rules_fired` array)
        // Assert: one `policy.rule_fired` event per fired rule name
        unimplemented!("one policy.rule_fired event per entry in rules_fired");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn deny_path_still_records_policy_rule_fired_for_matching_deny_rule() {
        // Arrange: explicit deny rule fires before SpiceDB
        // Assert: `policy.rule_fired` event present; `decision=deny` on span attributes
        unimplemented!("policy.rule_fired on deny path with decision=deny attribute");
    }
}

mod error_status {
    //! JSON-RPC errors set span status ERROR with stable error attributes.

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn policy_deny_sets_span_status_error_with_jsonrpc_code() {
        // Arrange: CEL deny returning Trogon JSON-RPC error envelope
        // Assert: span status ERROR; `error.code` == JSON-RPC code (e.g. -32100)
        unimplemented!("ERROR status and error.code from JSON-RPC code per reference-error-codes.md");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn error_span_includes_error_reason_from_data_reason() {
        // Arrange: application error with `data.reason` string
        // Assert: span attribute `error.reason` == `data.reason` (not free-form message)
        unimplemented!("error.reason from JSON-RPC data.reason on ERROR spans");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn backend_timeout_sets_error_status_with_backend_timeout_code() {
        // Arrange: backend NATS request times out
        // Assert: ERROR status; `error.code` matches backend_timeout allocation
        unimplemented!("backend timeout ERROR span with stable error.code");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn allow_path_span_status_is_unset_or_ok() {
        // Arrange: successful forward and JSON-RPC result
        // Assert: root span status not ERROR; no `error.code` attribute
        unimplemented!("successful allow path leaves span status OK/unset");
    }
}

mod exporter {
    //! OTLP exporters receive spans; tests assert via in-memory exporter when supported.

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn in_memory_otlp_exporter_receives_root_and_child_spans() {
        // Arrange: gateway test harness with trogon-telemetry in-memory OTLP exporter
        // Act: single ingress request through allow path
        // Assert: exporter buffer contains root span and expected child spans
        unimplemented!("in-memory OTLP exporter receives gateway spans");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn exported_root_span_trace_id_matches_jsonrpc_error_data_trace_id() {
        // Arrange: deny path returning JSON-RPC error with `data.trace_id`
        // Assert: OTLP root span trace_id (32 hex) equals error `data.trace_id`
        unimplemented!("trace_id correlation between OTLP export and JSON-RPC data.trace_id");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when OTel span shape per Pin 7 lands"]
    async fn exported_span_trace_id_matches_audit_envelope_trace_id() {
        // Arrange: JetStream audit publish for same request
        // Assert: audit envelope `trace_id` matches OTLP root span trace id
        unimplemented!("trace_id correlation between OTLP export and audit envelope");
    }
}

mod wasm_evaluate_span {
    //! Tier-3 WASM evaluate span shape (ADR 0032). OTLP/NATS harness cases remain ignored above.

    use trogon_mcp_gateway::observability::gateway_span_allowlist;
    use trogon_mcp_gateway::wasm::WASM_EVALUATE_SPAN_NAME;

    #[test]
    fn wasm_evaluate_span_name_matches_otel_wiring() {
        assert_eq!(WASM_EVALUATE_SPAN_NAME, "mcp.gateway.wasm.evaluate");
    }

    #[test]
    fn emitted_span_allowlist_includes_wasm_evaluate_and_handle_ingress() {
        let allowlist = gateway_span_allowlist();
        assert!(allowlist.contains(&WASM_EVALUATE_SPAN_NAME));
        assert!(allowlist.contains(&"mcp_gateway.handle_ingress"));
        assert!(allowlist.contains(&"mcp.gateway.plugin.call"));
    }
}
