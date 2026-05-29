//! Gateway JSON-RPC error code integration tests (scaffold).
//!
//! Scenario: Phase 1 pins gateway-emitted JSON-RPC error codes per Wire-Format Pin 6.
//! Each module covers one Trogon application error code, its trigger condition, and the
//! stable `error.data` shape clients must rely on (not `error.message` text).
//!
//! Cross-references:
//! - `docs/identity/reference-error-codes.md`
//! - `docs/identity/failure-mode-matrix.md`
//! - `reference-error-codes.md`
//!
//! Harness pattern: live NATS broker, `mcp_nats::Config`, `McpPrefix`, `trogon_nats::NatsAuth`,
//! `GatewaySettings`, and gateway ingress request/reply (see `e2e_nats_forward.rs`).
//!
//! Once error emission lands, remove `#[ignore]`, implement Arrange / Act / Assert, and verify
//! every response includes `data.trace_id` matching the inbound W3C `traceparent` trace-id.

use trogon_mcp_gateway::rpc_codes;

mod policy_deny {
    use super::rpc_codes;

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32100 policy_deny per Wire-Format Pin 6 lands"]
    async fn cel_rule_explicitly_denies_returns_policy_deny_code() {
        // Arrange: policy bundle with CEL rule returning deny(reason)
        // Act: tools/call through gateway ingress
        // Assert: error.code == rpc_codes::POLICY_DENY
        unimplemented!("verify error.code == {}", rpc_codes::POLICY_DENY);
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32100 policy_deny per Wire-Format Pin 6 lands"]
    async fn spicedb_deny_includes_rule_fired_and_reason() {
        // Arrange: SpiceDB CheckBulkPermissions returns denied for tools/call
        // Assert: data.rule_fired, data.reason, data.trace_id present
        unimplemented!("assert data.rule_fired, data.reason, data.trace_id");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32100 policy_deny per Wire-Format Pin 6 lands"]
    async fn hierarchical_merge_deny_wins_emits_policy_deny() {
        // Arrange: tiered policy merge where one tier denies
        unimplemented!("failure-mode-matrix row 1 deny path");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32100 policy_deny per Wire-Format Pin 6 lands"]
    async fn trace_id_matches_inbound_traceparent() {
        // Arrange: ingress request with traceparent header
        // Assert: data.trace_id equals W3C trace-id portion
        unimplemented!("trace_id correlation per reference-error-codes.md §2");
    }
}

mod policy_fault {
    #[tokio::test]
    #[ignore = "scaffold; implement when error -32101 policy_fault per Wire-Format Pin 6 lands"]
    async fn malformed_cel_returns_policy_fault_code() {
        // Arrange: bundle with syntactically invalid CEL
        // Assert: error.code == -32101
        unimplemented!("verify error.code == -32101");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32101 policy_fault per Wire-Format Pin 6 lands"]
    async fn cel_runtime_error_includes_tier_and_error() {
        // Arrange: CEL rule that divides by zero at runtime
        // Assert: data.tier == "cel", data.error is non-empty
        unimplemented!("assert data.tier and data.error");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32101 policy_fault per Wire-Format Pin 6 lands"]
    async fn wasm_trap_includes_tier_wasm() {
        // Arrange: Tier-3 WASM guest panics during evaluation
        // Assert: data.tier == "wasm"
        unimplemented!("failure-mode-matrix row 7");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32101 policy_fault per Wire-Format Pin 6 lands"]
    async fn trace_id_matches_inbound_traceparent() {
        unimplemented!("trace_id correlation per reference-error-codes.md §2");
    }
}

mod backend_timeout {
    use super::rpc_codes;

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32102 backend_timeout per Wire-Format Pin 6 lands"]
    async fn backend_exceeds_deadline_returns_backend_timeout_code() {
        // Arrange: backend subscriber absent or slow beyond operation timeout
        // Assert: error.code == rpc_codes::BACKEND_TIMEOUT
        unimplemented!("verify error.code == {}", rpc_codes::BACKEND_TIMEOUT);
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32102 backend_timeout per Wire-Format Pin 6 lands"]
    async fn data_includes_server_id_and_elapsed_ms() {
        // Assert: data.server_id matches target backend, data.elapsed_ms >= deadline
        unimplemented!("assert data.server_id and data.elapsed_ms");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32102 backend_timeout per Wire-Format Pin 6 lands"]
    async fn no_nats_reply_within_operation_timeout() {
        // Arrange: gateway forwards to mcp.server.{server_id}.> with no reply
        unimplemented!("failure-mode-matrix row 8");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32102 backend_timeout per Wire-Format Pin 6 lands"]
    async fn trace_id_matches_inbound_traceparent() {
        unimplemented!("trace_id correlation per reference-error-codes.md §2");
    }
}

mod backend_unreachable {
    use super::rpc_codes;

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32103 backend_unreachable per Wire-Format Pin 6 lands"]
    async fn no_consumer_on_queue_group_returns_backend_unreachable() {
        // Arrange: backend queue group with zero active consumers
        // Assert: error.code == rpc_codes::BACKEND_UNREACHABLE
        unimplemented!("verify error.code == {}", rpc_codes::BACKEND_UNREACHABLE);
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32103 backend_unreachable per Wire-Format Pin 6 lands"]
    async fn unknown_server_id_returns_backend_unreachable() {
        // Arrange: tools/call targeting unregistered server_id
        unimplemented!("assert data.server_id");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32103 backend_unreachable per Wire-Format Pin 6 lands"]
    async fn disabled_backend_returns_backend_unreachable() {
        // Arrange: registered but disabled backend
        unimplemented!("failure-mode-matrix row 9");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32103 backend_unreachable per Wire-Format Pin 6 lands"]
    async fn trace_id_matches_inbound_traceparent() {
        unimplemented!("trace_id correlation per reference-error-codes.md §2");
    }
}

mod schema_unknown {
    #[tokio::test]
    #[ignore = "scaffold; implement when error -32104 schema_unknown per Wire-Format Pin 6 lands"]
    async fn input_schema_missing_from_cache_and_fetch_fails() {
        // Arrange: redaction requires inputSchema; cache miss and fetch failure
        // Assert: error.code == -32104
        unimplemented!("verify error.code == -32104");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32104 schema_unknown per Wire-Format Pin 6 lands"]
    async fn data_includes_server_id_and_tool() {
        // Assert: data.server_id and data.tool identify the failing tool
        unimplemented!("assert data.server_id and data.tool");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32104 schema_unknown per Wire-Format Pin 6 lands"]
    async fn redaction_cannot_validate_without_schema() {
        // Arrange: enforce-mode redaction on $.params.* paths
        unimplemented!("schema publication failure path");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32104 schema_unknown per Wire-Format Pin 6 lands"]
    async fn trace_id_matches_inbound_traceparent() {
        unimplemented!("trace_id correlation per reference-error-codes.md §2");
    }
}

mod rate_limited {
    use super::rpc_codes;

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32105 rate_limited per Wire-Format Pin 6 lands"]
    async fn ingress_budget_exceeded_returns_rate_limited_code() {
        // Arrange: exceed ingress rate budget (see also rate_limit_caps.rs)
        // Assert: error.code == rpc_codes::RATE_LIMITED
        unimplemented!("verify error.code == {}", rpc_codes::RATE_LIMITED);
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32105 rate_limited per Wire-Format Pin 6 lands"]
    async fn data_includes_scope_and_retry_after_ms() {
        // Assert: data.scope and data.retry_after_ms present
        unimplemented!("assert data.scope and data.retry_after_ms");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32105 rate_limited per Wire-Format Pin 6 lands"]
    async fn per_tool_budget_exceeded() {
        // Arrange: per-tool inflight cap or rate budget exceeded
        unimplemented!("failure-mode-matrix row 10");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32105 rate_limited per Wire-Format Pin 6 lands"]
    async fn trace_id_matches_inbound_traceparent() {
        unimplemented!("trace_id correlation per reference-error-codes.md §2");
    }
}

mod auth_expired {
    use super::rpc_codes;

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32106 auth_expired per Wire-Format Pin 6 lands"]
    async fn jwt_expired_mid_session_returns_auth_expired_code() {
        // Arrange: bearer JWT with exp in the past at post-auth gate
        // Assert: error.code == rpc_codes::AUTH_EXPIRED
        unimplemented!("verify error.code == {}", rpc_codes::AUTH_EXPIRED);
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32106 auth_expired per Wire-Format Pin 6 lands"]
    async fn session_kv_revocation_returns_auth_expired() {
        // Arrange: session revoked in KV between initialize and tools/call
        unimplemented!("failure-mode-matrix row 13 JWT path");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32106 auth_expired per Wire-Format Pin 6 lands"]
    async fn data_includes_trace_id_only_beyond_code() {
        // Assert: no extra stable data fields beyond trace_id
        unimplemented!("reference-error-codes.md §3.1 auth_expired");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32106 auth_expired per Wire-Format Pin 6 lands"]
    async fn trace_id_matches_inbound_traceparent() {
        unimplemented!("trace_id correlation per reference-error-codes.md §2");
    }
}

mod authz_unreachable {
    use super::rpc_codes;

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32107 authz_unreachable per Wire-Format Pin 6 lands"]
    async fn spicedb_unreachable_returns_authz_unreachable_code() {
        // Arrange: SpiceDB endpoint down or gRPC transport failure
        // Assert: error.code == rpc_codes::AUTHZ_UNREACHABLE (not approval_required data)
        unimplemented!("verify error.code == {}", rpc_codes::AUTHZ_UNREACHABLE);
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32107 authz_unreachable per Wire-Format Pin 6 lands"]
    async fn data_includes_elapsed_ms() {
        // Assert: data.elapsed_ms reflects PDP deadline wait
        unimplemented!("assert data.elapsed_ms");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32107 authz_unreachable per Wire-Format Pin 6 lands"]
    async fn pdp_deadline_exceeded_fail_closed() {
        // Arrange: SpiceDB responds after gateway PDP deadline
        unimplemented!("failure-mode-matrix row 1");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32107 authz_unreachable per Wire-Format Pin 6 lands"]
    async fn trace_id_matches_inbound_traceparent() {
        unimplemented!("trace_id correlation per reference-error-codes.md §2");
    }
}

mod no_policy {
    #[tokio::test]
    #[ignore = "scaffold; implement when error -32108 no_policy per Wire-Format Pin 6 lands"]
    async fn bundle_not_loaded_default_deny_returns_no_policy() {
        // Arrange: no active bundle pointer for tenant with default-deny posture
        // Assert: error.code == -32108
        unimplemented!("verify error.code == -32108");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32108 no_policy per Wire-Format Pin 6 lands"]
    async fn no_active_bundle_pointer_for_tenant() {
        // Arrange: tenant without loaded policy bundle
        unimplemented!("failure-mode-matrix row 5");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32108 no_policy per Wire-Format Pin 6 lands"]
    async fn data_includes_trace_id_only_beyond_code() {
        // Assert: no extra stable data fields beyond trace_id
        unimplemented!("reference-error-codes.md §3.1 no_policy");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32108 no_policy per Wire-Format Pin 6 lands"]
    async fn trace_id_matches_inbound_traceparent() {
        unimplemented!("trace_id correlation per reference-error-codes.md §2");
    }
}

mod audience_mismatch {
    use super::rpc_codes;

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32109 audience_mismatch per Wire-Format Pin 6 lands"]
    async fn jwt_aud_mismatch_returns_audience_mismatch_code() {
        // Arrange: bearer JWT aud != gateway expected audience in enforce mode
        // Assert: error.code == rpc_codes::AUDIENCE_MISMATCH
        unimplemented!("verify error.code == {}", rpc_codes::AUDIENCE_MISMATCH);
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32109 audience_mismatch per Wire-Format Pin 6 lands"]
    async fn data_includes_expected_aud_and_actual_aud() {
        // Assert: data.expected_aud and data.actual_aud echo validation drift
        unimplemented!("assert data.expected_aud and data.actual_aud");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32109 audience_mismatch per Wire-Format Pin 6 lands"]
    async fn enforce_mode_rejects_drifted_aud() {
        // Arrange: egress mint or ingress validation detects aud drift
        unimplemented!("oauth-mcp-integration enforce path");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when error -32109 audience_mismatch per Wire-Format Pin 6 lands"]
    async fn trace_id_matches_inbound_traceparent() {
        unimplemented!("trace_id correlation per reference-error-codes.md §2");
    }
}
