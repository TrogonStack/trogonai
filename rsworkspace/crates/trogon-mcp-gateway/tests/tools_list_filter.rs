//! Per-item CEL filtering for MCP catalog list responses.
//!
//! Phase 2 re-evaluates the merged authorization rule once per catalogue entry on
//! `tools/list`, `resources/list`, and `prompts/list` before egress. Denied items
//! are omitted; the JSON-RPC envelope is otherwise unchanged.
//!
//! Cross-references:
//! - `docs/adr/0015-tools-list-filtering.md` (accepted decision)
//! - `docs/identity/tools-list-filtering.md` (algorithm, CEL bindings, audit fields)
//! - `MCP_GATEWAY_PLAN.md` Block E item 2
//!
//! Intended harness: NATS gateway worker with backend stub (`e2e_nats_forward.rs`
//! pattern — `mcp_nats::Config`, `McpPrefix`, `trogon_nats::NatsAuth`, `GatewaySettings`,
//! `AllowAllPermissionChecker`, policy bundle with list filter enabled). Filtering hook:
//! `trogon_mcp_gateway::policy::list_filter` (proposed).
//!
//! Once implemented, each test verifies:
//! - Partial denial trims the visible catalogue without JSON-RPC errors
//! - `response.list_filter_index` binds per candidate during re-evaluation
//! - Empty backend and all-denied responses succeed with empty arrays
//! - `resources/list` and `prompts/list` share the same per-item semantics
//! - Audit envelope emits `catalog_tools_filtered` (filtered_count) via mock publisher

mod tools_list_partial_denial {
    #[tokio::test]
    #[ignore = "scaffold; implement when list-filter per ADR 0015 lands"]
    async fn cel_rule_denies_two_of_five_backend_tools_client_sees_three() {
        // Arrange: NATS harness; backend stub returns five tools; CEL bundle denies two by name.
        // Act: client `tools/list` via `{prefix}.gateway.request.{server_id}.tools.list`.
        // Assert: JSON-RPC success; `result.tools` length == 3; denied names absent.
        unimplemented!("wire list_filter hook and policy bundle per ADR 0015");
    }
}

mod tools_list_edge_cases {
    #[tokio::test]
    #[ignore = "scaffold; implement when list-filter per ADR 0015 lands"]
    async fn empty_backend_tools_array_returns_empty_success() {
        // Arrange: backend stub returns `"tools": []`.
        // Act: client `tools/list` through gateway with list filter enabled.
        // Assert: success envelope; `result.tools` is empty; no JSON-RPC error.
        unimplemented!("empty catalogue passthrough per docs/identity/tools-list-filtering.md §7.4");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when list-filter per ADR 0015 lands"]
    async fn all_tools_denied_returns_empty_tools_array() {
        // Arrange: backend returns N tools; CEL rule denies every candidate.
        // Act: client `tools/list`.
        // Assert: JSON-RPC success; `result.tools` == []; no synthetic placeholders.
        unimplemented!("all-denied shaping per ADR 0015 response shape invariant");
    }
}

mod list_filter_index {
    #[tokio::test]
    #[ignore = "scaffold; implement when list-filter per ADR 0015 lands"]
    async fn cel_rule_evaluates_with_response_list_filter_index_per_item() {
        // Arrange: policy expr references `response.list_filter_index`; backend returns ordered tools.
        // Act: gateway filters list response via `policy::list_filter`.
        // Assert: each evaluation sees the zero-based index matching the candidate position.
        unimplemented!("response.list_filter_index binding per ADR 0015 evaluation contract");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when list-filter per ADR 0015 lands"]
    async fn list_filter_index_differs_across_sibling_catalog_entries() {
        // Arrange: rule keeps only even indices (`response.list_filter_index % 2 == 0`).
        // Act: `tools/list` with four backend tools.
        // Assert: client sees tools at indices 0 and 2 only.
        unimplemented!("indexed per-item re-evaluation per docs/identity/tools-list-filtering.md");
    }
}

mod catalog_list_parity {
    #[tokio::test]
    #[ignore = "scaffold; implement when list-filter per ADR 0015 lands"]
    async fn tools_list_per_item_cel_filter_matches_call_authorization_rule() {
        // Arrange: same merged CEL rule gates `tools/call` and filters `tools/list`.
        // Act: list request with mixed allow/deny tools.
        // Assert: visible tools match call-time authorization intent.
        unimplemented!("tools/list parity per MCP_GATEWAY_PLAN Block E item 2");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when list-filter per ADR 0015 lands"]
    async fn resources_list_per_item_cel_filter_matches_call_authorization_rule() {
        // Arrange: backend `resources/list` with `mcp.resource.uri` bindings; CEL denies subset.
        // Act: client `resources/list`.
        // Assert: filtered `result.resources` array; same OR semantics as tools/list.
        unimplemented!("resources/list shaping per ADR 0015 catalog method table");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when list-filter per ADR 0015 lands"]
    async fn prompts_list_per_item_cel_filter_matches_call_authorization_rule() {
        // Arrange: backend `prompts/list` with `mcp.prompt.name` bindings; CEL denies subset.
        // Act: client `prompts/list`.
        // Assert: filtered `result.prompts` array; same OR semantics as tools/list.
        unimplemented!("prompts/list shaping per ADR 0015 catalog method table");
    }
}

mod audit_filtered_count {
    #[tokio::test]
    #[ignore = "scaffold; implement when list-filter per ADR 0015 lands"]
    async fn audit_envelope_records_catalog_tools_filtered_count() {
        // Arrange: mock audit publisher; backend five tools; CEL denies two.
        // Act: `tools/list` through gateway with audit enabled.
        // Assert: published envelope has `catalog_tools_filtered == 2` (total - visible).
        unimplemented!("AuditEnvelope catalog_* fields per docs/identity/tools-list-filtering.md §8");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when list-filter per ADR 0015 lands"]
    async fn mock_audit_publisher_receives_filtered_count_after_list_shaping() {
        // Arrange: subscribe to `{prefix}.audit.*.request.tools_list`; stub backend catalogue.
        // Act: shaped list response after partial denial.
        // Assert: audit subject payload includes filtered_count and policy_version.
        unimplemented!("mock audit publisher capture per ADR 0015 observability section");
    }
}
