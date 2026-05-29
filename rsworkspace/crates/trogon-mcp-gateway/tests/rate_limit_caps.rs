//! Integration test scaffold for Phase 2 per-server / per-tenant inflight caps and
//! per-caller rate budgets (Wire-Format Pin 9, Block E item 7).
//!
//! Once ADR 0012 lands, these tests exercise the live NATS gateway path using the
//! same harness as `e2e_nats_forward.rs` (`mcp_nats::Config`, `McpPrefix`,
//! `trogon_nats::NatsAuth`, `GatewaySettings`, slow backend stubs, JWT ingress).
//! Rate-limit wiring will reference `trogon_mcp_gateway::throttle::*` (scaffold module;
//! inflight semaphores and cluster rate budgets are TBD).
//!
//! Cross-references:
//! - `docs/adr/0012-rate-limit-state.md` — two-tier state placement (Tier 1 inflight, Tier 2 KV budgets)
//! - `docs/identity/rate-limiting.md` — placement layers, `-32105` contract, audit semantics
//! - `docs/identity/reference-rate-defaults.md` — Pin 9 defaults, `retry_after_ms`, header echo
//! - `docs/identity/reference-error-codes.md` — `-32105 rate_limited` JSON-RPC shape
//! - `MCP_GATEWAY_PLAN.md` Wire-Format Pin 9 + Block E item 7
//!
//! Verification contract (implement when un-ignored):
//! - Per-server inflight cap N=2: third concurrent request returns `-32105` with `data.scope = "server"`.
//! - Per-tenant inflight: same pattern with `data.scope = "tenant"`.
//! - Per-caller rate budget: 3 req / 1 s window; 4th returns `-32105` with `data.scope = "caller"` and
//!   `data.retry_after_ms > 0`.
//! - Distinct `jwt.sub` values do not share a caller budget.
//! - NATS reply header `retry-after-ms` mirrors `data.retry_after_ms` (proposed; see reference-rate-defaults §4).
//! - After the budget window elapses, the caller succeeds again.
//! - Audit envelope publishes `outcome = rate_limited` with matching scope.

use trogon_mcp_gateway::rpc_codes;

const RATE_LIMITED: i32 = rpc_codes::RATE_LIMITED;

/// Harness notes shared by all modules (see `tests/e2e_nats_forward.rs`).
///
/// Arrange: NATS broker, `McpPrefix`, slow backend subscriber on `{prefix}.server.{id}.tools.list`,
/// `GatewaySettings` with rate-limit config (TBD on `GatewaySettings` / bundle KV), JWT for caller.
/// Act: concurrent or sequential `request_with_headers` to `{prefix}.gateway.request.{id}.tools.list`.
/// Assert: JSON-RPC error code, `data.scope`, `data.retry_after_ms`, optional NATS headers, audit subject.
mod harness {
    use std::time::Duration;

    #[allow(dead_code)]
    pub const TEST_SERVER_INFLIGHT_CAP: u32 = 2;
    #[allow(dead_code)]
    pub const TEST_TENANT_INFLIGHT_CAP: u32 = 2;
    pub const TEST_CALLER_BUDGET: u32 = 3;
    pub const TEST_CALLER_WINDOW: Duration = Duration::from_secs(1);
    #[allow(dead_code)]
    pub const BACKEND_HOLD: Duration = Duration::from_secs(5);
}

mod per_server_inflight {

    #[tokio::test]
    #[ignore = "scaffold; implement when rate-limit per ADR 0012 lands"]
    async fn third_concurrent_request_returns_rate_limited_server_scope() {
        // Arrange: cap N=2 per server_id (test override; Pin 9 default 256).
        // Act: launch 3 concurrent tools/list with backend hold so all stay inflight.
        // Assert: first two succeed; third JSON-RPC error code -32105, data.scope == "server".
        unimplemented!("wire Tier-1 server inflight semaphore per ADR 0012");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when rate-limit per ADR 0012 lands"]
    async fn server_inflight_cap_releases_slot_on_backend_completion() {
        // Arrange: cap N=2, two slow in-flight requests.
        // Act: complete one backend reply, send a third request.
        // Assert: third succeeds (slot released); no -32105.
        unimplemented!("inflight slot release after backend terminal response");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when rate-limit per ADR 0012 lands"]
    async fn under_cap_requests_succeed_without_rate_limit() {
        // Arrange: cap N=2, fast backend.
        // Act: two sequential successful tools/list.
        // Assert: both return result envelopes; no error.code == -32105.
        unimplemented!("baseline success under server inflight cap");
    }
}

mod per_tenant_inflight {

    #[tokio::test]
    #[ignore = "scaffold; implement when rate-limit per ADR 0012 lands"]
    async fn third_concurrent_request_returns_rate_limited_tenant_scope() {
        // Arrange: tenant inflight cap N=2 (test override; Pin 9 default 4096).
        // Act: 3 concurrent requests for same tenant, different server_ids if needed.
        // Assert: third returns -32105 with data.scope == "tenant".
        unimplemented!("wire Tier-1 tenant inflight semaphore per ADR 0012");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when rate-limit per ADR 0012 lands"]
    async fn tenant_inflight_counts_across_server_ids() {
        // Arrange: cap N=2 for tenant "acme".
        // Act: two concurrent requests to server_a and server_b under same tenant JWT.
        // Assert: third request to either server returns scope "tenant".
        unimplemented!("tenant inflight is tenant-wide not per-server");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when rate-limit per ADR 0012 lands"]
    async fn distinct_tenants_do_not_share_inflight_budget() {
        // Arrange: cap N=2 per tenant.
        // Act: saturate tenant_a; send request as tenant_b.
        // Assert: tenant_b request succeeds despite tenant_a saturation.
        unimplemented!("tenant inflight isolation");
    }
}

mod per_caller_rate_budget {
    use super::{RATE_LIMITED};
    use super::harness::{TEST_CALLER_BUDGET, TEST_CALLER_WINDOW};

    #[tokio::test]
    #[ignore = "scaffold; implement when rate-limit per ADR 0012 lands"]
    async fn fourth_request_in_one_second_returns_caller_scope() {
        // Arrange: caller budget 3 req / 1 s (Tier-2 hybrid per ADR 0012).
        // Act: 4 rapid tools/list from same jwt.sub within TEST_CALLER_WINDOW.
        // Assert: 4th error.code == RATE_LIMITED (-32105), data.scope == "caller".
        let _ = (RATE_LIMITED, TEST_CALLER_BUDGET, TEST_CALLER_WINDOW);
        unimplemented!("wire per-caller rate budget via rate.acquire / KV sync");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when rate-limit per ADR 0012 lands"]
    async fn rate_limited_response_includes_positive_retry_after_ms() {
        // Arrange: exhaust caller budget (3 req / 1 s).
        // Act: one more request within the window.
        // Assert: error.data.retry_after_ms is present and > 0.
        unimplemented!("retry_after_ms on caller scope deny");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when rate-limit per ADR 0012 lands"]
    async fn first_three_requests_within_budget_succeed() {
        // Arrange: budget 3 req / 1 s, fast backend.
        // Act: three sequential tools/list within window.
        // Assert: all return result envelopes.
        unimplemented!("baseline success under caller rate budget");
    }
}

mod caller_isolation {
    use super::harness::{TEST_CALLER_BUDGET, TEST_CALLER_WINDOW};

    #[tokio::test]
    #[ignore = "scaffold; implement when rate-limit per ADR 0012 lands"]
    async fn distinct_callers_do_not_share_rate_budget() {
        // Arrange: budget 3 req / 1 s per jwt.sub.
        // Act: exhaust budget for caller_alice; request as caller_bob.
        // Assert: bob succeeds; alice still rate-limited on next attempt.
        let _ = (TEST_CALLER_BUDGET, TEST_CALLER_WINDOW);
        unimplemented!("caller budget keyed by jwt.sub");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when rate-limit per ADR 0012 lands"]
    async fn same_caller_sub_reuses_budget_across_requests() {
        // Arrange: budget 3 req / 1 s.
        // Act: three requests as alice, then fourth as alice.
        // Assert: fourth denied; alice cannot bypass by changing server_id or tool.
        unimplemented!("caller budget is per jwt.sub cluster-wide");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when rate-limit per ADR 0012 lands"]
    async fn exhausted_caller_does_not_affect_other_tenant_callers() {
        // Arrange: alice@tenant_a exhausted; bob@tenant_b fresh budget.
        // Act: bob sends tools/list.
        // Assert: success; confirms key includes tenant isolation where applicable.
        unimplemented!("caller budget tenant prefix per rate-limiting.md");
    }
}

mod retry_after_header {

    #[tokio::test]
    #[ignore = "scaffold; implement when rate-limit per ADR 0012 lands"]
    async fn nats_reply_sets_retry_after_ms_header() {
        // Proposed (reference-rate-defaults.md §4): gateway SHOULD set NATS header
        // `retry-after-ms` on every -32105 where retry_after_ms is computed.
        // Arrange: trigger any rate limit deny (caller budget simplest).
        // Act: inspect NATS response headers on ingress reply.
        // Assert: header "retry-after-ms" present with integer string > 0.
        unimplemented!("NATS retry-after-ms header echo (proposed)");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when rate-limit per ADR 0012 lands"]
    async fn header_retry_after_ms_matches_jsonrpc_data_field() {
        // Proposed: same integer as data.retry_after_ms in JSON-RPC body.
        // Arrange: caller budget deny.
        // Assert: parse header and body; values equal.
        unimplemented!("header/body retry_after_ms parity (proposed)");
    }
}

mod window_recovery {
    use super::harness::TEST_CALLER_WINDOW;

    #[tokio::test]
    #[ignore = "scaffold; implement when rate-limit per ADR 0012 lands"]
    async fn caller_can_request_after_window_elapses() {
        // Arrange: exhaust 3 req / 1 s budget.
        // Act: sleep TEST_CALLER_WINDOW + margin; send tools/list.
        // Assert: success result envelope (budget reset).
        let _ = TEST_CALLER_WINDOW;
        unimplemented!("sliding/fixed window recovery for caller budget");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when rate-limit per ADR 0012 lands"]
    async fn retry_after_ms_bounds_recovery_sleep() {
        // Arrange: deny on 4th request; record data.retry_after_ms.
        // Act: sleep retry_after_ms (with small margin); retry.
        // Assert: success without -32105.
        unimplemented!("client-visible retry_after_ms is a hint not a lease");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when rate-limit per ADR 0012 lands"]
    async fn immediate_retry_after_deny_still_rate_limited() {
        // Arrange: deny on 4th request within window.
        // Act: immediate retry without sleep.
        // Assert: still -32105 scope caller.
        unimplemented!("no budget recovery before window elapses");
    }
}

mod audit_rate_limited {

    #[tokio::test]
    #[ignore = "scaffold; implement when rate-limit per ADR 0012 lands"]
    async fn rate_limited_publishes_audit_with_outcome_rate_limited() {
        // Arrange: subscribe to `{prefix}.audit.deny.request.*` (or normalized rate_limited subject).
        // Act: trigger server inflight deny.
        // Assert: audit envelope outcome == "rate_limited" (reference-audit-envelope.md).
        unimplemented!("audit publish on rate limit deny");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when rate-limit per ADR 0012 lands"]
    async fn audit_envelope_includes_rate_limit_scope() {
        // Arrange: audit subscriber; trigger tenant-scope deny.
        // Assert: envelope carries scope matching JSON-RPC data.scope ("tenant").
        unimplemented!("audit scope field mirrors JSON-RPC data.scope");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when rate-limit per ADR 0012 lands"]
    async fn audit_envelope_includes_retry_after_ms() {
        // Arrange: audit subscriber; trigger caller-scope deny.
        // Assert: audit retry_after_ms mirrors JSON-RPC data.retry_after_ms.
        unimplemented!("audit retry_after_ms field (proposed additive)");
    }
}
