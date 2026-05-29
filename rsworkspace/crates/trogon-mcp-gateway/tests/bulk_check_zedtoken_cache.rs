//! Integration scaffold for BulkCheckPermission + ZedToken cache on `tools/list`.
//!
//! Phase 2 shapes catalog responses by batching per-tool permission checks into one
//! SpiceDB `CheckBulkPermissions` call and caching the returned ZedToken plus allow/deny
//! results keyed by `{session_id, principal, permission}`.
//!
//! Cross-references:
//! - `docs/adr/0014-bulk-check-zedtoken-cache.md`
//! - `docs/identity/bulk-check-permission.md`
//! - `MCP_GATEWAY_PLAN.md` Block E item 3
//!
//! Target cache module (not implemented yet): `trogon_mcp_gateway::spicedb::zedtoken_cache`.
//!
//! Once implemented, these tests verify:
//! - Cold `tools/list` invokes BulkCheckPermission and stores the session ZedToken.
//! - Repeat `tools/list` within TTL reuses the cached token without SpiceDB RPCs.
//! - `notifications/tools/list_changed` invalidates affected session cache entries.
//! - TTL expiry evicts entries and forces a fresh BulkCheck on the next list.
//! - Cache entries are isolated per principal (`jwt.sub`); no cross-subject sharing.

use std::time::Duration;

use mcp_nats::{Config as McpConfig, McpPrefix};
use trogon_mcp_gateway::gateway::GatewaySettings;
use trogon_nats::{NatsAuth, NatsConfig};

/// Expected cache surface once ADR 0014 lands (`trogon_mcp_gateway::spicedb::zedtoken_cache`).
#[allow(dead_code)]
const ZEDTOKEN_CACHE_MODULE: &str = "trogon_mcp_gateway::spicedb::zedtoken_cache";

/// Harness fixture shape reused from `e2e_nats_forward.rs` when wiring live NATS + gateway.
#[allow(dead_code)]
struct ZedTokenCacheHarness {
    nats_conf: NatsConfig,
    mcp_conf: McpConfig,
    prefix: McpPrefix,
    settings: GatewaySettings,
}

#[allow(dead_code)]
impl ZedTokenCacheHarness {
    fn new(prefix_token: &str, queue_group: &str) -> Self {
        let url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
        let nats_conf = NatsConfig::new(vec![url], NatsAuth::None);
        let prefix = McpPrefix::new(prefix_token).expect("test prefix shape");
        let mcp_conf = McpConfig::new(prefix.clone(), nats_conf.clone())
            .with_operation_timeout(Duration::from_secs(15));
        let settings = GatewaySettings {
            queue_group: queue_group.into(),
            audit_stream_name: "MCP_AUDIT_ZEDTOKEN_CACHE_SCAFFOLD".into(),
            init_audit_stream: false,
            mcp: mcp_conf.clone(),
            jwt: trogon_mcp_gateway::jwt::JwtValidator::disabled().expect("jwt ingress off"),
            egress: None,
            chain_resolver: None,
            rate_limit: None,
        };
        Self {
            nats_conf,
            mcp_conf,
            prefix,
            settings,
        }
    }
}

mod cold_path {
    //! First `tools/list` in a session: BulkCheckPermission invoked, ZedToken stored.

    #[tokio::test]
    #[ignore = "scaffold; implement when ZedToken cache lands per ADR 0014"]
    async fn first_tools_list_invokes_bulk_check_permission() {
        // Arrange: live NATS backend returning N tools; SpiceDB mock recording RPC count.
        // Act: send `tools/list` with fresh `mcp-session-id` and verified JWT principal.
        // Assert: exactly one CheckBulkPermissions with N miss items; 0 cache hits.
        unimplemented!("wire SpiceDB test double; expect trogon_mcp_gateway::spicedb::zedtoken_cache cold miss");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when ZedToken cache lands per ADR 0014"]
    async fn first_tools_list_stores_zedtoken_for_session_principal_permission() {
        // Arrange: bulk response includes `checked_at` token for `(session_id, principal, invoke)`.
        // Act: complete first shaped `tools/list`.
        // Assert: ZedTokenCache entry at key `v1|zed|{session_id}|{principal}|invoke`.
        unimplemented!("inspect trogon_mcp_gateway::spicedb::zedtoken_cache after cold list");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when ZedToken cache lands per ADR 0014"]
    async fn first_tools_list_uses_minimize_latency_when_no_cached_token() {
        // Arrange: empty ZedToken cache for session tuple.
        // Act: cold `tools/list` shaping path.
        // Assert: BulkCheck uses `minimize_latency` consistency (no `at_least_as_fresh` header).
        unimplemented!("assert SpiceDB consistency header on first list per bulk-check-permission.md");
    }
}

mod cache_hit_within_ttl {
    //! Repeat `tools/list` within TTL: SpiceDB not invoked, cached ZedToken reused.

    #[tokio::test]
    #[ignore = "scaffold; implement when ZedToken cache lands per ADR 0014"]
    async fn second_tools_list_within_ttl_skips_spicedb_rpc() {
        // Arrange: warm cache from prior `tools/list`; TTL not elapsed.
        // Act: second `tools/list` with same session, principal, and unchanged catalog.
        // Assert: SpiceDB RPC count remains 0; `spicedb_cache_hit_total` increments.
        unimplemented!("count SpiceDB calls across two list requests within TTL");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when ZedToken cache lands per ADR 0014"]
    async fn second_tools_list_within_ttl_uses_cached_zedtoken_for_consistency() {
        // Arrange: cached ZedToken from first bulk response.
        // Act: second list when all tool results hit cache.
        // Assert: gateway serves shaped list without BulkCheck; session latest token unchanged.
        unimplemented!("verify trogon_mcp_gateway::spicedb::zedtoken_cache token reuse per ADR 0014");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when ZedToken cache lands per ADR 0014"]
    async fn repeat_tools_list_returns_same_filtered_catalog_when_graph_unchanged() {
        // Arrange: N-tool backend catalog; first list stored allow/deny results.
        // Act: repeat list within TTL.
        // Assert: identical filtered `result.tools` envelope; no backend or PDP side effects.
        unimplemented!("compare shaped tools/list payloads on cache hit path");
    }
}

mod list_changed_invalidation {
    //! `notifications/tools/list_changed` drops cache entries; next list re-checks SpiceDB.

    #[tokio::test]
    #[ignore = "scaffold; implement when ZedToken cache lands per ADR 0014"]
    async fn list_changed_notification_drops_session_zedtoken_entries() {
        // Arrange: warm ZedToken + result cache for `(session_id, server_id)`.
        // Act: publish `notifications/tools/list_changed` for that server.
        // Assert: trogon_mcp_gateway::spicedb::zedtoken_cache has no entries for session.
        unimplemented!("hook schema_cache/invalidate.rs extension per ADR 0014");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when ZedToken cache lands per ADR 0014"]
    async fn tools_list_after_list_changed_reinvokes_bulk_check() {
        // Arrange: prior warm cache invalidated by list_changed.
        // Act: next `tools/list`.
        // Assert: exactly one BulkCheckPermission; cache miss metrics emitted.
        unimplemented!("assert cold PDP path after list_changed invalidation");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when ZedToken cache lands per ADR 0014"]
    async fn list_changed_for_other_server_does_not_invalidate_unrelated_session_cache() {
        // Arrange: two sessions on different server_id bindings; warm both caches.
        // Act: list_changed for server A only.
        // Assert: session B cache entries remain; session A entries dropped.
        unimplemented!("scope invalidation to affected server_id per tools-list-filtering.md");
    }
}

mod ttl_expiry {
    //! Per-entry TTL eviction forces a fresh BulkCheck on the next `tools/list`.

    #[tokio::test]
    #[ignore = "scaffold; implement when ZedToken cache lands per ADR 0014"]
    async fn cache_entry_evicted_after_configured_ttl_elapses() {
        // Arrange: warm cache; advance clock past `min(zed_expiration, MCP_GATEWAY_SPICEDB_ZEDTOKEN_CACHE_TTL_SECS)`.
        // Act: probe ZedTokenCache before and after TTL.
        // Assert: entry absent; `spicedb_cache_eviction_total{reason="ttl"}` incremented.
        unimplemented!("drive trogon_mcp_gateway::spicedb::zedtoken_cache TTL expiry");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when ZedToken cache lands per ADR 0014"]
    async fn tools_list_after_ttl_expiry_reinvokes_bulk_check() {
        // Arrange: expired cache entry for session tuple.
        // Act: `tools/list` after TTL.
        // Assert: BulkCheck invoked; new ZedToken stored.
        unimplemented!("assert cold PDP path after TTL safety net per ADR 0014");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when ZedToken cache lands per ADR 0014"]
    async fn ttl_clamped_to_min_of_zed_expiration_and_configured_max() {
        // Arrange: bulk response token metadata + env `MCP_GATEWAY_SPICEDB_ZEDTOKEN_CACHE_TTL_SECS`.
        // Act: insert cache entry.
        // Assert: entry TTL equals `min(zed_expiration, configured_max_ttl)` from ADR 0014.
        unimplemented!("verify TTL clamp on trogon_mcp_gateway::spicedb::zedtoken_cache insert");
    }
}

mod principal_isolation {
    //! Different `jwt.sub` values must not share ZedToken or permission result cache entries.

    #[tokio::test]
    #[ignore = "scaffold; implement when ZedToken cache lands per ADR 0014"]
    async fn different_jwt_sub_does_not_share_zedtoken_cache_entry() {
        // Arrange: principal A completes cold `tools/list`; principal B same session header.
        // Act: principal B lists tools.
        // Assert: separate cache keys `{session_id, trogon/principal:{sub}, invoke}`; B misses.
        unimplemented!("cross-principal isolation for trogon_mcp_gateway::spicedb::zedtoken_cache");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when ZedToken cache lands per ADR 0014"]
    async fn different_jwt_sub_does_not_hit_other_principals_permission_results() {
        // Arrange: principal A allowed on tool T; principal B denied on same tool in SpiceDB.
        // Act: B calls `tools/list` after A warmed cache.
        // Assert: B receives deny-shaped catalog; no cache hit from A's result sub-keys.
        unimplemented!("verify per-principal result sub-keys per bulk-check-permission.md");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when ZedToken cache lands per ADR 0014"]
    async fn same_session_id_different_principal_gets_independent_cache_namespace() {
        // Arrange: shared `mcp-session-id` header but distinct verified JWT `sub` claims.
        // Act: interleaved `tools/list` from both principals.
        // Assert: two independent ZedToken cursors; neither principal observes the other's token.
        unimplemented!("principal segment in cache key must include normalized caller sub");
    }
}
