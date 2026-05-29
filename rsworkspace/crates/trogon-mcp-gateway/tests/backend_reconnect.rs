//! Backend reconnect integration tests (scaffold).
//!
//! Phase 2 ships backend reconnect handling with a per-server version bump that invalidates
//! schema and STS egress token caches. When a backend disconnects, in-flight gateway requests
//! for that `server_id` fail with `-32103` `backend_unreachable`. On reconnect, the gateway
//! debounces rapid flaps, bumps `server.version`, evicts that server's caches, and emits
//! `mcp.backend.reconnect` telemetry. The next `tools/call` refetches schemas via a fresh
//! `tools/list` without disturbing other servers' cache entries.
//!
//! Cross-references:
//! - `MCP_GATEWAY_PLAN.md` Block C item 3 (schema cache invalidation on reconnect)
//! - `MCP_GATEWAY_PLAN.md` Block E (server reconnect handling, version bump, debounce)
//! - `docs/identity/reference-error-codes.md` (`-32103` `backend_unreachable`)
//! - `docs/identity/integration-touchpoints.md` (per-server egress mesh-token cache)
//!
//! Harness pattern: live NATS broker, `mcp_nats::Config`, `McpPrefix`, `trogon_nats::NatsAuth`,
//! `GatewaySettings`, `AllowAllPermissionChecker`, and `trogon_mcp_gateway::run`
//! (see `e2e_nats_forward.rs`).
//!
//! Once backend reconnect + version bump lands, remove `#[ignore]` on each test and fill in
//! Arrange / Act / Assert.

use trogon_mcp_gateway::rpc_codes;

mod disconnect_inflight {
    //! In-flight requests for a disconnected backend receive `-32103` `backend_unreachable`.

    use super::rpc_codes;

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn inflight_tools_call_returns_backend_unreachable_on_disconnect() {
        // Arrange: gateway running; backend stub subscribed for `server-a`; warm path with one
        //          in-flight `tools/call` before tearing down backend NATS subscription.
        // Act: drop backend consumer / simulate disconnect while request pending.
        // Assert: JSON-RPC error.code == rpc_codes::BACKEND_UNREACHABLE (-32103).
        unimplemented!(
            "in-flight tools/call -> error.code == {}",
            rpc_codes::BACKEND_UNREACHABLE
        );
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn inflight_tools_list_returns_backend_unreachable_on_disconnect() {
        // Arrange: gateway + backend for `server-a`; client request in flight on tools/list lane.
        // Act: backend disconnect before reply.
        // Assert: error.code == -32103; error.data.server_id == "server-a".
        unimplemented!("in-flight tools/list fails closed on disconnect");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn multiple_inflight_requests_all_fail_with_backend_unreachable() {
        // Arrange: N concurrent ingress requests targeting same `server_id` during disconnect.
        // Act: backend goes away with all N in flight.
        // Assert: every response carries -32103; no partial success.
        unimplemented!("all in-flight requests for disconnected server fail");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn backend_unreachable_includes_server_id_in_error_data() {
        // Arrange: disconnect `server-a` with one pending request.
        // Act: await error envelope.
        // Assert: error.data.server_id == "server-a" per reference-error-codes.md §3.1.
        unimplemented!("backend_unreachable data.server_id present");
    }
}

mod reconnect_cache_evict {
    //! Reconnect bumps `server.version` and evicts that server's schema cache entries.

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn reconnect_bumps_server_version() {
        // Arrange: record `server.version` for `server-a` after initial connection.
        // Act: simulate backend disconnect + stable reconnect.
        // Assert: `server.version` incremented exactly once (post-debounce).
        unimplemented!("version bump on stable reconnect per Block E");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn reconnect_evicts_schema_cache_for_that_server() {
        // Arrange: warm schema cache for `server-a` via `tools/list`.
        // Act: reconnect `server-a` (version bump).
        // Assert: all `(server-a, _)` `SchemaCacheKey` entries removed per Block C item 3.
        unimplemented!("schema cache evicted for reconnected server");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn reconnect_does_not_serve_stale_cached_schemas() {
        // Arrange: warm cache, force reconnect without immediate `tools/list`.
        // Act: `tools/call` targeting `server-a`.
        // Assert: pre-reconnect schema not used; refetch path engaged.
        unimplemented!("stale schema not served after version bump");
    }
}

mod tools_list_refetch {
    //! After reconnect, the next `tools/call` triggers a fresh `tools/list` to repopulate schemas.

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn tools_call_after_reconnect_triggers_fresh_tools_list() {
        // Arrange: warm cache, reconnect `server-a`, instrument backend `tools.list` subject.
        // Act: first `tools/call` post-reconnect.
        // Assert: backend receives exactly one `tools/list` before call proceeds.
        unimplemented!("post-reconnect tools/call sniffs tools/list");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn tools_list_refetch_repopulates_schema_cache() {
        // Arrange: cold cache after reconnect; backend stub returns updated tool schemas.
        // Act: `tools/call` (implicit refetch).
        // Assert: `SchemaCache::get` succeeds for new `(server_id, schema_hash)` entries.
        unimplemented!("refetch repopulates schema cache");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn tools_call_after_refetch_hits_repopulated_cache() {
        // Arrange: complete one post-reconnect `tools/call` (refetch + populate).
        // Act: second `tools/call` for same tool.
        // Assert: cache hit; no additional backend `tools/list`.
        unimplemented!("second call hits repopulated cache");
    }
}

mod isolation {
    //! Server A reconnect must not invalidate server B schema cache or version state.

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn server_a_reconnect_preserves_server_b_schema_cache() {
        // Arrange: warm schema cache for `server-a` and `server-b`.
        // Act: reconnect only `server-a`.
        // Assert: `server-b` cache entries remain; `SchemaCache::get` still hits for `server-b`.
        unimplemented!("cross-server schema cache isolation on reconnect");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn server_a_version_bump_does_not_touch_server_b_version() {
        // Arrange: record versions for `server-a` and `server-b`.
        // Act: reconnect `server-a`.
        // Assert: `server-b.version` unchanged.
        unimplemented!("version bump scoped to reconnected server");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn server_b_tools_call_still_hits_cache_after_server_a_reconnect() {
        // Arrange: warm both servers; reconnect `server-a` only.
        // Act: `tools/call` for `server-b`.
        // Assert: cache hit; no backend `tools/list` for `server-b`.
        unimplemented!("unaffected server continues cache hits");
    }
}

mod debounce {
    //! Rapid backend flaps debounce to one version bump per stable connection.

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn rapid_backend_flap_produces_single_version_bump() {
        // Arrange: record initial `server.version`; flap backend disconnect/reconnect rapidly (< debounce window).
        // Act: settle on stable connection.
        // Assert: version incremented once, not per flap.
        unimplemented!("debounced version bump on reconnect storm");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn debounce_waits_for_stable_connection_before_bump() {
        // Arrange: oscillating backend presence within debounce interval.
        // Act: observe version and cache state during flapping.
        // Assert: no version bump until connection stable per Block E debounce policy.
        unimplemented!("version bump deferred until stable reconnect");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn debounced_reconnect_does_not_evict_cache_on_transient_disconnect() {
        // Arrange: warm cache; brief disconnect shorter than debounce threshold.
        // Act: backend returns before debounce fires.
        // Assert: cache entries retained; version unchanged.
        unimplemented!("transient flap does not invalidate cache");
    }
}

mod telemetry {
    //! Reconnect emits `mcp.backend.reconnect` with server identity and version transition.

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn reconnect_emits_mcp_backend_reconnect_event() {
        // Arrange: telemetry sink / OTel span exporter wired to gateway.
        // Act: stable reconnect for `server-a`.
        // Assert: event name `mcp.backend.reconnect` observed once.
        unimplemented!("telemetry event mcp.backend.reconnect");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn reconnect_telemetry_includes_server_id_and_versions() {
        // Arrange: record pre-reconnect version; reconnect `server-a`.
        // Act: capture telemetry payload.
        // Assert: attributes `server_id`, `previous_version`, `new_version` present and consistent.
        unimplemented!("reconnect telemetry carries server_id and version fields");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn reconnect_telemetry_fires_once_per_debounced_bump() {
        // Arrange: reconnect storm with debounce; telemetry collector counting events.
        // Act: flap then stable reconnect.
        // Assert: exactly one `mcp.backend.reconnect` event (matches single version bump).
        unimplemented!("telemetry deduplicated with debounced version bump");
    }
}

mod token_invalidate {
    //! Per-server STS-minted egress token cache is invalidated on reconnect (token-versioning).

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn reconnect_invalidates_sts_minted_token_cache_for_server() {
        // Arrange: warm egress mesh-token cache for `server-a` via successful `tools/call`.
        // Act: reconnect `server-a` (version bump).
        // Assert: cached token entry for `(server-a, …)` evicted per server token-versioning.
        unimplemented!("STS token cache invalidated on reconnect");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn tools_call_after_reconnect_mints_fresh_mesh_token() {
        // Arrange: stub STS exchange counting mint requests; warm token cache, then reconnect.
        // Act: first `tools/call` post-reconnect.
        // Assert: STS mint invoked once; egress uses new mesh JWT (not pre-reconnect cache hit).
        unimplemented!("post-reconnect egress mint bypasses stale token cache");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when backend reconnect + version bump per MCP_GATEWAY_PLAN.md Block E lands"]
    async fn token_cache_invalidated_only_for_reconnected_server() {
        // Arrange: warm egress token cache for `server-a` and `server-b`.
        // Act: reconnect only `server-a`.
        // Assert: `server-b` token cache entry still valid; `server-a` requires remint.
        unimplemented!("token cache isolation across servers on reconnect");
    }
}
