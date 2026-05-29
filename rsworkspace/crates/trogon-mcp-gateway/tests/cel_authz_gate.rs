//! Integration tests for the Phase 1 CEL SpiceDB authorization gate.
//!
//! The gate expression `mcp.method == "tools/call" || mcp.method == "resources/read"`
//! selects when the SpiceDB-backed [`trogon_mcp_gateway::authz::PermissionChecker`] hook
//! runs on ingress JSON-RPC requests. Methods outside that set must bypass SpiceDB entirely.
//!
//! Cross-refs:
//! - `MCP_GATEWAY_PLAN.md` Block D Phase 1 (CEL gate + SpiceDB hook)
//! - `docs/adr/0008-policy-dsl.md` (CEL policy DSL)
//! - `docs/identity/policy-dsl-choice.md` (DSL selection rationale)
//!
//! Once implemented, each test will drive the gateway harness (`mcp_nats::Config`, `McpPrefix`,
//! `trogon_nats::NatsAuth`, `GatewaySettings`, etc.) and assert whether the checker was invoked
//! via a spy/counter hook exposed by the gate (`authz` / `policy` modules).

#![allow(unused_imports)]

use std::sync::Arc;
use std::time::Duration;

use mcp_nats::{Config as McpConfig, McpPrefix};
use trogon_mcp_gateway::authz::AllowAllPermissionChecker;
use trogon_mcp_gateway::gateway::GatewaySettings;
use trogon_mcp_gateway::policy::SpicedbGatePolicy;
use trogon_nats::{NatsAuth, NatsConfig, connect};

mod gate_selects_spicedb {
    use super::*;

    #[tokio::test]
    #[ignore = "scaffold; implement when MCP_GATEWAY_PLAN Block D Phase 1 CEL gate exposes spy / counter hook (ADR 0008)"]
    async fn gate_evaluates_true_for_tools_call_invokes_spicedb_checker() {
        unimplemented!("scaffold; populate when CEL gate exposes spy / counter hook");
        // Arrange: NATS broker, gateway with SpicedbGatePolicy::phase1_hardcoded(), counting checker.
        // Act: publish JSON-RPC tools/call on ingress subject.
        // Assert: checker invocation count == 1.
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when MCP_GATEWAY_PLAN Block D Phase 1 CEL gate exposes spy / counter hook (ADR 0008)"]
    async fn gate_evaluates_true_for_resources_read_invokes_spicedb_checker() {
        unimplemented!("scaffold; populate when CEL gate exposes spy / counter hook");
        // Arrange: NATS broker, gateway with SpicedbGatePolicy::phase1_hardcoded(), counting checker.
        // Act: publish JSON-RPC resources/read on ingress subject.
        // Assert: checker invocation count == 1.
    }
}

mod gate_bypasses_spicedb {
    use super::*;

    #[tokio::test]
    #[ignore = "scaffold; implement when MCP_GATEWAY_PLAN Block D Phase 1 CEL gate exposes spy / counter hook (ADR 0008)"]
    async fn gate_evaluates_false_for_tools_list_bypasses_spicedb_checker() {
        unimplemented!("scaffold; populate when CEL gate exposes spy / counter hook");
        // Arrange: NATS broker, gateway with AllowAllPermissionChecker replaced by counting spy.
        // Act: publish JSON-RPC tools/list on ingress subject.
        // Assert: checker invocation count == 0; request still forwards.
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when MCP_GATEWAY_PLAN Block D Phase 1 CEL gate exposes spy / counter hook (ADR 0008)"]
    async fn gate_evaluates_false_for_initialize_bypasses_spicedb_checker() {
        unimplemented!("scaffold; populate when CEL gate exposes spy / counter hook");
        // Arrange: NATS broker, gateway with counting checker spy.
        // Act: publish JSON-RPC initialize on ingress subject.
        // Assert: checker invocation count == 0; request still forwards.
    }
}
