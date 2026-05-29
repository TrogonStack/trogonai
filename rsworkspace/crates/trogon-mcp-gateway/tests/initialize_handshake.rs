//! Gateway-terminated `initialize` handshake integration tests (Phase 2 / Wire-Format Pin 5).
//!
//! Scenario: the gateway answers client `initialize` itself, records client metadata in session KV,
//! and lazily forwards backend `initialize` on first routed call. Virtual `tools/list` fans out
//! parallel backend init before merge; `notifications/initialized` reaches only warmed backends.
//!
//! Cross-references:
//! - `docs/identity/reference-initialize.md` (edge handshake, KV keys, lazy forward)
//! - `docs/identity/mcp-session-model.md` (session lifecycle, `mcp-session-id`)
//! - `reference-initialize.md` (gateway-terminated by default)
//!
//! Implementation targets (not yet present): `trogon_mcp_gateway::gateway::session`,
//! `trogon_mcp_gateway::gateway::initialize`.
//!
//! Once implemented, these tests verify the full initialize handshake contract against a live NATS
//! broker using the existing gateway harness (`GatewaySettings`, `mcp_nats::Config`, `McpPrefix`,
//! `trogon_nats::NatsAuth`, etc.).

#![allow(unused_imports)]

use std::sync::Arc;
use std::time::Duration;

use mcp_nats::{Config as McpConfig, McpPrefix};
use trogon_mcp_gateway::authz::AllowAllPermissionChecker;
use trogon_mcp_gateway::gateway::GatewaySettings;
use trogon_mcp_gateway::jwt::JwtValidator;
use trogon_nats::{NatsAuth, NatsConfig};
use uuid::Uuid;

mod gateway_terminated_init {
    //! Client `initialize` is answered at the gateway edge; backends are not contacted.

    #[tokio::test]
    #[ignore = "scaffold; implement when initialize handshake per Wire-Format Pin 5 lands"]
    async fn initialize_returns_gateway_server_info_name() {
        // Arrange: start gateway with stub backend that would fail if contacted during init.
        // Act: client sends `initialize` on `{prefix}.gateway.request.{server_id}.initialize`.
        // Assert: result.serverInfo.name == "trogon-mcp-gateway".
        unimplemented!("verify gateway serverInfo.name per reference-initialize.md");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when initialize handshake per Wire-Format Pin 5 lands"]
    async fn initialize_returns_union_capabilities() {
        // Arrange: federation members expose disjoint capability flags.
        // Act: client `initialize`.
        // Assert: gateway capabilities object is the union of federation features.
        unimplemented!("verify union capabilities per Wire-Format Pin 5");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when initialize handshake per Wire-Format Pin 5 lands"]
    async fn initialize_does_not_contact_backend() {
        // Arrange: subscribe to `mcp.server.{id}.initialize`; start gateway.
        // Act: client edge `initialize`.
        // Assert: no backend initialize subject receives a message.
        unimplemented!("backend must not be contacted during edge initialize");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when initialize handshake per Wire-Format Pin 5 lands"]
    async fn initialize_response_includes_mcp_session_id_header() {
        // Arrange: start gateway; client omits `mcp-session-id` on first init.
        // Act: client `initialize`.
        // Assert: reply headers contain newly minted `mcp-session-id`.
        unimplemented!("verify mcp-session-id header per mcp-session-model.md");
    }
}

mod session_kv_client_info {
    //! Client `clientInfo` and `protocolVersion` are persisted under the issued session id.

    #[tokio::test]
    #[ignore = "scaffold; implement when initialize handshake per Wire-Format Pin 5 lands"]
    async fn client_info_recorded_in_session_kv() {
        // Arrange: start gateway with session KV bucket.
        // Act: client `initialize` with distinct clientInfo.
        // Assert: KV record at session key contains matching clientInfo.
        unimplemented!("session KV clientInfo per reference-initialize.md section 5");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when initialize handshake per Wire-Format Pin 5 lands"]
    async fn protocol_version_recorded_in_session_kv() {
        // Arrange: start gateway with session KV bucket.
        // Act: client `initialize` with negotiated protocolVersion param.
        // Assert: KV record stores chosen protocolVersion.
        unimplemented!("session KV protocolVersion per reference-initialize.md");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when initialize handshake per Wire-Format Pin 5 lands"]
    async fn session_kv_keyed_by_issued_mcp_session_id() {
        // Arrange: start gateway.
        // Act: two sequential edge `initialize` calls (re-init semantics).
        // Assert: each response's `mcp-session-id` maps to a distinct KV entry.
        unimplemented!("KV keyed by mcp-session-id per mcp-session-model.md");
    }
}

mod lazy_backend_init {
    //! First routed call to a non-virtual backend triggers lazy backend `initialize`.

    #[tokio::test]
    #[ignore = "scaffold; implement when initialize handshake per Wire-Format Pin 5 lands"]
    async fn first_tools_call_forwards_initialize_to_backend() {
        // Arrange: edge init complete; backend subscribed on `mcp.server.{id}.initialize`.
        // Act: first `tools/call` to non-virtual target.
        // Assert: backend receives `initialize` before tools/call payload.
        unimplemented!("lazy backend initialize on first tools/call");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when initialize handshake per Wire-Format Pin 5 lands"]
    async fn lazy_initialize_precedes_actual_tools_call() {
        // Arrange: record backend message order via subscription.
        // Act: first `tools/call`.
        // Assert: initialize request timestamp/order precedes tools/call forward.
        unimplemented!("initialize must precede forwarded method call");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when initialize handshake per Wire-Format Pin 5 lands"]
    async fn no_backend_initialize_before_first_routed_call() {
        // Arrange: edge init only; backend listener on initialize subject.
        // Act: wait without sending operation-phase RPC.
        // Assert: backend initialize subject idle.
        unimplemented!("no eager backend warm-up after edge init");
    }
}

mod backend_init_cache {
    //! Backend `InitializeResult` is cached per (session, server_id); not re-sent.

    #[tokio::test]
    #[ignore = "scaffold; implement when initialize handshake per Wire-Format Pin 5 lands"]
    async fn backend_initialize_response_cached_in_session_kv() {
        // Arrange: first tools/call completes backend init.
        // Act: read session KV for (session_id, backend_id).
        // Assert: cached backend InitializeResult present.
        unimplemented!("cache backend init in session KV");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when initialize handshake per Wire-Format Pin 5 lands"]
    async fn subsequent_tools_call_skips_backend_initialize() {
        // Arrange: one completed tools/call warmed the backend.
        // Act: second tools/call to same backend.
        // Assert: backend initialize subject receives no additional message.
        unimplemented!("reuse cached backend init on subsequent calls");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when initialize handshake per Wire-Format Pin 5 lands"]
    async fn cached_backend_capabilities_not_re_exposed_to_client() {
        // Arrange: backend returns capabilities distinct from gateway union.
        // Act: client operation-phase RPC after lazy init.
        // Assert: client never receives backend-only capability surface.
        unimplemented!("backend init cached internally only per reference-initialize.md");
    }
}

mod virtual_tools_list_parallel_init {
    //! Virtual-target `tools/list` initializes all federation members in parallel before merge.

    #[tokio::test]
    #[ignore = "scaffold; implement when initialize handshake per Wire-Format Pin 5 lands"]
    async fn virtual_tools_list_initializes_all_federation_members() {
        // Arrange: virtual target spanning N backends; none warmed.
        // Act: client `tools/list` on virtual server_id.
        // Assert: each member receives exactly one backend initialize.
        unimplemented!("parallel init all members before virtual tools/list merge");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when initialize handshake per Wire-Format Pin 5 lands"]
    async fn parallel_initialize_completes_before_tools_list_merge() {
        // Arrange: staggered backend init latencies.
        // Act: virtual `tools/list`.
        // Assert: merged result arrives only after all backend inits succeed.
        unimplemented!("merge blocked until parallel init completes");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when initialize handshake per Wire-Format Pin 5 lands"]
    async fn virtual_tools_list_does_not_skip_uninitialized_member() {
        // Arrange: virtual target with one member never contacted before list.
        // Act: virtual `tools/list`.
        // Assert: that member is initialized and included in merged tool catalog.
        unimplemented!("no uninitialized member in virtual tools/list output");
    }
}

mod initialized_notification_routing {
    //! `notifications/initialized` is forwarded only to backends already initialized.

    #[tokio::test]
    #[ignore = "scaffold; implement when initialize handshake per Wire-Format Pin 5 lands"]
    async fn initialized_notification_forwarded_to_ready_backends() {
        // Arrange: edge init; one backend warmed via prior tools/call.
        // Act: client sends `notifications/initialized`.
        // Assert: warmed backend receives the notification.
        unimplemented!("forward initialized to ready backends only");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when initialize handshake per Wire-Format Pin 5 lands"]
    async fn uninitialized_backend_does_not_receive_initialized() {
        // Arrange: edge init; federation member never routed to.
        // Act: client sends `notifications/initialized`.
        // Assert: cold backend receives no notification.
        unimplemented!("skip uninitialized backends for notifications/initialized");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when initialize handshake per Wire-Format Pin 5 lands"]
    async fn initialized_notification_after_partial_warmup() {
        // Arrange: two backends; only one warmed.
        // Act: client sends `notifications/initialized`.
        // Assert: exactly the warmed backend receives the notification.
        unimplemented!("partial federation warmup routing per mcp-session-model.md");
    }
}
