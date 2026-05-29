//! WASM host ABI integration tests (scaffold).
//!
//! Scenario: Phase 3 ships WASM-based policy/rewrite extensions per
//! `docs/identity/reference-host-abi.md`. These tests cover the gateway-provided
//! host import surface that WASM policy bundles call during evaluation.
//!
//! Cross-references:
//! - `docs/identity/reference-host-abi.md` (host function signatures, capability gating)
//! - `docs/identity/mcp-policy-wit-sketch.md` (WIT import shapes, `target_wit` pin)
//! - `MCP_GATEWAY_PLAN.md` Block F (Phase 3 WASM runtime + linker)
//!
//! Harness pattern: live NATS broker, `mcp_nats::Config`, `McpPrefix`, `trogon_nats::NatsAuth`,
//! `GatewaySettings`, signed WASM fixture bundle, and gateway ingress request/reply
//! (see `e2e_nats_forward.rs`).
//!
//! Once the WASM host ABI lands, remove `#[ignore]`, implement Arrange / Act / Assert, and verify:
//! - `host.log(level, msg)` records log lines with `module_id` attribute
//! - `host.metric_inc(name, value, labels)` exposes counters on `/metrics`
//! - `host.get_header(key)` / `host.set_header(key, value)` read and rewrite NATS headers
//! - `host.jwt_claim(key)` returns verified JWT claims (read-only)
//! - `host.deadline_ms()` returns remaining request time budget
//! - ABI version negotiation accepts `host_abi=1` and rejects unknown versions at load
//! - WASM traps (OOB memory, CPU budget) deny with `-32101` `policy_fault`

/// Wire-format Pin 6 policy evaluation fault (not yet exported from `rpc_codes`).
const POLICY_FAULT: i32 = -32_101;

mod host_log {
    //! `host.log(level, msg)` forwards structured log lines to gateway telemetry.

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_log_emitted_with_module_id_attribute() {
        // Arrange: WASM fixture guest calls host.log(info, "policy evaluated")
        // Act: tools/call through gateway with Tier-3 WASM bundle active
        // Assert: gateway log/tracing span includes module_id == bundle instance id
        unimplemented!("host.log records message with module_id attribute");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_log_levels_map_to_tracing_levels() {
        // Arrange: guest emits trace/debug/info/warn/error via host.log
        // Assert: each level maps to the corresponding tracing severity
        unimplemented!("host.log level enum maps to tracing levels");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_log_does_not_affect_policy_decision() {
        // Arrange: guest returns allow after host.log side effect
        // Assert: decision is allow; log is side-effect only per mcp-policy-wit-sketch.md
        unimplemented!("host.log is non-deterministic side effect only");
    }
}

mod host_metrics {
    //! `host.metric_inc(name, value, labels)` increments gateway counters.

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_metric_counter_appears_in_metrics_endpoint() {
        // Arrange: guest calls host.metric_inc("policy.eval", 1, {})
        // Act: scrape gateway /metrics (or in-process meter export)
        // Assert: counter series `policy.eval` incremented by 1
        unimplemented!("host.metric_inc exposes counter on /metrics");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_metric_labels_included_in_series() {
        // Arrange: guest passes labels { "tenant": "acme", "bundle": "risk-v1" }
        // Assert: exported metric includes label dimensions on the series
        unimplemented!("host.metric_inc labels attached to counter series");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_metric_inc_requires_declared_capability() {
        // Arrange: bundle manifest omits host.metric-inc from capabilities.imports
        // Act: guest attempts host.metric_inc at runtime
        // Assert: -32101 policy_fault undeclared_host_capability per reference-host-abi.md SS12
        unimplemented!("undeclared metric_inc returns policy_fault");
    }
}

mod host_headers {
    //! `host.get_header(key)` reads ingress NATS headers; `host.set_header(key, value)` rewrites egress.

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_get_header_reads_nats_request_header() {
        // Arrange: ingress request with custom header mcp-purpose: debug
        // Act: guest calls host.get_header("mcp-purpose")
        // Assert: guest receives "debug"
        unimplemented!("host.get_header reads NATS request context header");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_get_header_missing_key_returns_empty() {
        // Arrange: ingress without the requested header
        // Assert: host.get_header returns empty / none, not an error
        unimplemented!("host.get_header missing key returns empty");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_set_header_rewrites_egress_on_allowlist() {
        // Arrange: bundle allowlists mcp-trace-context; guest sets new value
        // Act: allowed tools/call proceeds to backend
        // Assert: backend NATS message carries rewritten header value
        unimplemented!("host.set_header rewrites allowlisted egress header");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_set_header_denied_for_non_allowlisted_key() {
        // Arrange: guest attempts host.set_header on undeclared / non-allowlisted key
        // Assert: host returns error or evaluation faults with policy_fault
        unimplemented!("host.set_header rejected outside allowlist");
    }
}

mod host_jwt {
    //! `host.jwt_claim(key)` exposes verified JWT claims (read-only).

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_jwt_claim_reads_verified_sub_claim() {
        // Arrange: ingress JWT with sub=alice; guest calls host.jwt_claim("sub")
        // Assert: guest receives "alice" from verified claims, not raw token bytes
        unimplemented!("host.jwt_claim returns verified claim value");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_jwt_claim_unknown_key_returns_empty() {
        // Arrange: JWT without custom claim; guest calls host.jwt_claim("missing")
        // Assert: empty / none, not policy_fault
        unimplemented!("host.jwt_claim missing key returns empty");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_jwt_claim_is_read_only() {
        // Arrange: guest attempts to mutate claim via host import (no set variant exists)
        // Assert: only get path is linkable; no write host function exported
        unimplemented!("host.jwt_claim has no write counterpart");
    }
}

mod host_deadline {
    //! `host.deadline_ms()` returns remaining request time budget.

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_deadline_ms_returns_remaining_budget() {
        // Arrange: ingress with mcp-deadline-unix-ms header set 5s ahead
        // Act: guest calls host.deadline_ms() early in evaluate
        // Assert: returned ms is positive and less than total budget
        unimplemented!("host.deadline_ms returns remaining request budget");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_deadline_ms_clamped_to_zero_when_expired() {
        // Arrange: deadline header in the past or elapsed during slow guest work
        // Assert: host.deadline_ms() returns 0
        unimplemented!("host.deadline_ms returns 0 when budget exhausted");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_host_io_timeouts_clamped_to_deadline() {
        // Arrange: guest calls host import with long internal timeout
        // Assert: host clamps to min(remaining deadline, configured default) per reference-host-abi.md SS2
        unimplemented!("host I/O timeouts clamped to request deadline");
    }
}

mod abi_versioning {
    //! ABI version negotiation: module declares `host_abi=1`; gateway accepts compatible versions.

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_module_host_abi_v1_accepted_by_gateway() {
        // Arrange: bundle manifest declares host_abi=1, target_wit trogon:mcp-policy@0.1.0
        // Act: gateway loads and links component
        // Assert: instance pooled; evaluate callable
        unimplemented!("host_abi=1 accepted when gateway host_abi>=1");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_gateway_rejects_unknown_abi_version_at_load() {
        // Arrange: bundle declares host_abi=99 (future / unknown)
        // Act: bundle activation / hot-swap
        // Assert: load fails; prior revision remains active per wasm-bundle-format.md
        unimplemented!("unknown host_abi rejected at link/load time");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_linker_requires_matching_target_wit() {
        // Arrange: bundle target_wit mismatches gateway linker pin
        // Assert: Wasmtime link fails; bundle not activated
        unimplemented!("target_wit pin enforced at link time");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_manifest_declares_import_superset() {
        // Arrange: guest uses host.log but manifest omits it from capabilities.imports
        // Assert: static scan rejects bundle at activation per reference-host-abi.md SS12.3
        unimplemented!("manifest must declare superset of reachable host imports");
    }
}

mod wasm_traps {
    //! WASM sandbox traps map to `-32101` `policy_fault` and deny the request.

    use super::POLICY_FAULT;

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_oob_memory_access_trapped_policy_fault() {
        // Arrange: guest fixture performs out-of-bounds linear memory access
        // Act: tools/call through gateway
        // Assert: error.code == POLICY_FAULT; request not forwarded to backend
        unimplemented!("OOB memory trap -> error.code == {}", POLICY_FAULT);
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_cpu_budget_exceeded_trapped_policy_fault() {
        // Arrange: guest infinite loop or heavy compute exceeding fuel/budget
        // Act: tools/call through gateway
        // Assert: trap; error.code == POLICY_FAULT
        unimplemented!("CPU budget trap -> error.code == {}", POLICY_FAULT);
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_trap_includes_tier_wasm_in_error_data() {
        // Arrange: guest trap during evaluate
        // Assert: error.data.tier == "wasm" per failure-mode-matrix.md
        unimplemented!("trap response includes data.tier == wasm");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when WASM host ABI per docs/identity/reference-host-abi.md lands"]
    async fn wasm_trap_drops_side_effect_journal() {
        // Arrange: guest calls audit.emit then traps before returning decision
        // Assert: no audit merge, no partial external effects per reference-host-abi.md SS13
        unimplemented!("trap drops per-evaluation effect journal");
    }
}
