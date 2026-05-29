//! Integration scaffold for virtual-MCP federation routing (Wire-Format Pin 4).
//!
//! Phase 2 multiplexes backend MCP servers behind edge `virtual-{id}` targets. Clients
//! send federated identifiers `{target}::{native}`; the gateway splits on the first
//! configured separator (default `::`), routes egress to `mcp.server.{backend}.*`, and
//! rewrites JSON-RPC params to the native segment only.
//!
//! Cross-references:
//! - [reference-virtual-mcp.md](../../../docs/identity/reference-virtual-mcp.md) (splitting,
//!   `tools/list` merge, resource URIs, failure codes)
//! - [MCP_GATEWAY_PLAN.md](../../../MCP_GATEWAY_PLAN.md) Wire-Format Pin 4 and
//!   "Virtual MCP (federation) in subjects"
//!
//! Implementation target (proposed): `trogon_mcp_gateway::gateway::virtual_mcp`.
//! Harness types: `mcp_nats::Config`, `McpPrefix`, `trogon_nats::NatsAuth`,
//! `trogon_mcp_gateway::gateway::GatewaySettings` (see `e2e_nats_forward.rs`).
//!
//! Once routing lands: remove `#[ignore]`, wire NATS fixtures, and assert ingress/egress
//! subjects plus rewritten `params`.

#![allow(unused_imports)]

use std::sync::Arc;
use std::time::Duration;

use mcp_nats::{Config as McpConfig, McpPrefix};
use trogon_mcp_gateway::authz::AllowAllPermissionChecker;
use trogon_mcp_gateway::gateway::GatewaySettings;
use trogon_nats::{NatsAuth, NatsConfig};

mod tools_call_routing {
    //! `tools/call` on a virtual server: first `::` split, member egress subject, native `params.name`.
    //!
    //! Spec: [reference-virtual-mcp.md §4/§8.2](../../../docs/identity/reference-virtual-mcp.md).

    #[tokio::test]
    #[ignore = "scaffold; implement when virtual-MCP routing per Wire-Format Pin 4 lands"]
    async fn tools_call_splits_federated_name_on_first_separator() {
        // Arrange: edge `virtual-default`, client `params.name = "github::create_issue"`.
        // Act: publish `mcp.gateway.request.virtual-default.tools.call`.
        // Assert: gateway resolves `target_prefix=github`, `native_name=create_issue`.
        unimplemented!("virtual-MCP tools/call split_once per reference-virtual-mcp.md §4");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when virtual-MCP routing per Wire-Format Pin 4 lands"]
    async fn tools_call_routes_egress_to_member_backend_subject() {
        // Arrange: virtual server with member `github` -> backend `github`.
        // Act: federated `tools/call` for `github::create_issue`.
        // Assert: egress subject `{prefix}.server.github.tools.call` (not `virtual-*`).
        unimplemented!("virtual-MCP egress subject per MCP_GATEWAY_PLAN.md Virtual MCP in subjects");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when virtual-MCP routing per Wire-Format Pin 4 lands"]
    async fn tools_call_rewrites_params_name_to_native_only() {
        // Arrange: ingress JSON-RPC `params.name = "github::create_issue"`.
        // Act: gateway forwards to member backend.
        // Assert: backend receives `params.name = "create_issue"` (separator stripped).
        unimplemented!("virtual-MCP params rewrite per reference-virtual-mcp.md §4");
    }
}

mod non_virtual_default_target {
    //! Non-virtual targets: tool names without `::` pass through unchanged (no federation split).
    //!
    //! Spec: federation applies only to `virtual-{id}` edge targets
    //! ([reference-virtual-mcp.md §1.2](../../../docs/identity/reference-virtual-mcp.md)).

    #[tokio::test]
    #[ignore = "scaffold; implement when virtual-MCP routing per Wire-Format Pin 4 lands"]
    async fn tools_call_without_separator_on_physical_server_unchanged() {
        // Arrange: edge `server_id=github` (non-virtual), `params.name = "create_issue"`.
        // Act: `tools/call` through gateway.
        // Assert: egress `mcp.server.github.tools.call` with `params.name` still `create_issue`.
        unimplemented!("non-virtual tools/call passthrough per reference-virtual-mcp.md §1.2");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when virtual-MCP routing per Wire-Format Pin 4 lands"]
    async fn tools_call_plain_name_preserves_ingress_and_egress_server_id() {
        // Arrange: default/physical target, no federated prefix in name.
        // Act: forward path.
        // Assert: ingress and egress `server_id` segment identical; no split attempted.
        unimplemented!("default target unchanged routing per Wire-Format Pin 4 non-virtual path");
    }
}

mod multi_separator_split {
    //! Multiple `::` in federated name: split once; native segment retains inner separators.
    //!
    //! Spec: [reference-virtual-mcp.md §3.2/§4](../../../docs/identity/reference-virtual-mcp.md).

    #[tokio::test]
    #[ignore = "scaffold; implement when virtual-MCP routing per Wire-Format Pin 4 lands"]
    async fn tools_call_vendor_namespace_tool_splits_on_first_separator_only() {
        // Arrange: `params.name = "vendor::namespace::tool"`.
        // Act: virtual-server `tools/call`.
        // Assert: `target_prefix=vendor`, `native_name=namespace::tool`.
        unimplemented!("first-separator-only split per reference-virtual-mcp.md §4");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when virtual-MCP routing per Wire-Format Pin 4 lands"]
    async fn tools_call_egress_params_name_retains_remaining_double_colon_segments() {
        // Arrange: federated `vendor::namespace::tool`, member backend `vendor`.
        // Act: egress to `mcp.server.vendor.tools.call`.
        // Assert: forwarded `params.name = "namespace::tool"` verbatim.
        unimplemented!("native_name preserves subsequent :: per reference-virtual-mcp.md §3.2");
    }
}

mod tools_list_federation {
    //! Virtual `tools/list`: parallel member init, merge, prefix each `name` with `{target}::`.
    //!
    //! Spec: [reference-virtual-mcp.md §6](../../../docs/identity/reference-virtual-mcp.md).

    #[tokio::test]
    #[ignore = "scaffold; implement when virtual-MCP routing per Wire-Format Pin 4 lands"]
    async fn tools_list_initializes_uninitialized_members_in_parallel() {
        // Arrange: virtual server members `github`, `linear`, `notion`; session without prior init.
        // Act: `mcp.gateway.request.virtual-default.tools.list`.
        // Assert: parallel `initialize` (or equivalent) per member before list fan-out.
        unimplemented!("parallel member init per MCP_GATEWAY_PLAN.md lazy virtual init");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when virtual-MCP routing per Wire-Format Pin 4 lands"]
    async fn tools_list_merges_member_catalogue_arrays() {
        // Arrange: backends return disjoint tool sets.
        // Act: federated `tools/list`.
        // Assert: single merged `result.tools` array to client.
        unimplemented!("tools/list merge per reference-virtual-mcp.md §6");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when virtual-MCP routing per Wire-Format Pin 4 lands"]
    async fn tools_list_prefixes_each_tool_name_with_target_separator() {
        // Arrange: member `github` exposes `create_issue`.
        // Act: virtual `tools/list`.
        // Assert: client sees `name: "github::create_issue"` (default separator `::`).
        unimplemented!("tools/list name prefix per Wire-Format Pin 4");
    }
}

mod unknown_target_errors {
    //! Unknown `target_prefix` after split -> `-32103` `backend_unreachable`.
    //!
    //! Spec: [reference-virtual-mcp.md §9.1](../../../docs/identity/reference-virtual-mcp.md).

    use trogon_mcp_gateway::rpc_codes;

    #[tokio::test]
    #[ignore = "scaffold; implement when virtual-MCP routing per Wire-Format Pin 4 lands"]
    async fn tools_call_unknown_target_prefix_returns_backend_unreachable() {
        // Arrange: virtual server without member `unknown`.
        // Act: `params.name = "unknown::create_issue"`.
        // Assert: JSON-RPC error code `rpc_codes::BACKEND_UNREACHABLE` (-32103).
        assert_eq!(rpc_codes::BACKEND_UNREACHABLE, -32_103);
        unimplemented!("unknown target -32103 per reference-virtual-mcp.md §9.1");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when virtual-MCP routing per Wire-Format Pin 4 lands"]
    async fn tools_call_unknown_prefix_carries_backend_unreachable_symbol() {
        // Arrange: unregistered `target_prefix` after split.
        // Act: virtual `tools/call`.
        // Assert: `error.message` / audit `decision_reason` = `backend_unreachable`.
        assert_eq!(rpc_codes::BACKEND_UNREACHABLE, -32_103);
        unimplemented!("backend_unreachable symbol per MCP_GATEWAY_PLAN.md §6");
    }
}

mod resource_uri_routing {
    //! `resources/read` applies the same first-separator split to federated URIs.
    //!
    //! Spec: [reference-virtual-mcp.md §7.1](../../../docs/identity/reference-virtual-mcp.md).

    #[tokio::test]
    #[ignore = "scaffold; implement when virtual-MCP routing per Wire-Format Pin 4 lands"]
    async fn resources_read_splits_github_issue_uri_on_first_separator() {
        // Arrange: `params.uri = "github::issue://owner/repo/123"`.
        // Act: virtual-server `resources/read`.
        // Assert: `target_prefix=github`, native `issue://owner/repo/123`.
        unimplemented!("resource URI split per reference-virtual-mcp.md §7.1");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when virtual-MCP routing per Wire-Format Pin 4 lands"]
    async fn resources_read_egress_uri_is_native_scheme_only() {
        // Arrange: federated URI `github::issue://owner/repo/123`, member `github`.
        // Act: egress `mcp.server.github.resources.read`.
        // Assert: backend `params.uri = "issue://owner/repo/123"` (no `github::` prefix).
        unimplemented!("resource URI egress rewrite per reference-virtual-mcp.md §7.1");
    }
}
