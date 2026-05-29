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

use std::sync::atomic::Ordering;
use std::time::Duration;

use mcp_nats::{Config as McpConfig, McpPrefix};
use trogon_mcp_gateway::gateway::GatewaySettings;
use trogon_mcp_gateway::spicedb::{
    CacheKeyParams, ResourceKey, ZedTokenCache, ZedTokenCacheConfig,
};
use trogon_nats::{NatsAuth, NatsConfig};

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

    use super::*;

    #[tokio::test]
    async fn first_tools_list_stores_zedtoken_for_session_principal_permission() {
        let cache = ZedTokenCache::default();
        let params = CacheKeyParams::new("sess-cold", "trogon/principal:alice", "invoke");

        cache
            .insert_zed_token(&params, "zed-from-bulk".into(), None)
            .await;

        assert_eq!(
            cache.get_zed_token(&params).await.as_deref(),
            Some("zed-from-bulk")
        );
        assert_eq!(
            ZedTokenCache::zed_key(&params),
            "v1|zed|sess-cold|trogon/principal:alice|invoke"
        );
    }

    #[tokio::test]
    async fn first_tools_list_uses_minimize_latency_when_no_cached_token() {
        let cache = ZedTokenCache::default();
        let params = CacheKeyParams::new("sess-cold", "trogon/principal:alice", "invoke");
        assert!(cache.get_zed_token(&params).await.is_none());
    }

    #[tokio::test]
    #[ignore = "needs live NATS + SpiceDB mock to count CheckBulkPermissions RPCs on cold tools/list"]
    async fn first_tools_list_invokes_bulk_check_permission() {
        unimplemented!("wire SpiceDB test double against gateway tools/list shaping path");
    }
}

mod cache_hit_within_ttl {
    //! Repeat `tools/list` within TTL: SpiceDB not invoked, cached ZedToken reused.

    use super::*;

    #[tokio::test]
    async fn second_tools_list_within_ttl_uses_cached_zedtoken_for_consistency() {
        let cache = ZedTokenCache::default();
        let params = CacheKeyParams::new("sess-warm", "trogon/principal:alice", "invoke");
        let resource = ResourceKey::new("trogon/mcp_tool", "filesystem|read_file");

        cache
            .insert_zed_token(&params, "tok-warm".into(), None)
            .await;
        cache
            .insert_results(&params, "tok-warm", &[(resource.clone(), true)], None)
            .await;

        assert_eq!(cache.get_zed_token(&params).await.as_deref(), Some("tok-warm"));
        assert_eq!(
            cache
                .get_result(&params, &resource, Some("tok-warm"))
                .await,
            Some(true)
        );
    }

    #[tokio::test]
    async fn repeat_tools_list_returns_same_filtered_catalog_when_graph_unchanged() {
        let cache = ZedTokenCache::default();
        let params = CacheKeyParams::new("sess-warm", "trogon/principal:alice", "invoke");
        let allowed = ResourceKey::new("trogon/mcp_tool", "filesystem|read_file");
        let denied = ResourceKey::new("trogon/mcp_tool", "filesystem|deploy");

        cache
            .insert_zed_token(&params, "tok-warm".into(), None)
            .await;
        cache
            .insert_results(
                &params,
                "tok-warm",
                &[(allowed.clone(), true), (denied.clone(), false)],
                None,
            )
            .await;

        assert_eq!(
            cache
                .get_result(&params, &allowed, Some("tok-warm"))
                .await,
            Some(true)
        );
        assert_eq!(
            cache
                .get_result(&params, &denied, Some("tok-warm"))
                .await,
            Some(false)
        );
    }

    #[tokio::test]
    #[ignore = "needs live NATS + SpiceDB mock to assert zero RPC count on repeat tools/list"]
    async fn second_tools_list_within_ttl_skips_spicedb_rpc() {
        unimplemented!("count SpiceDB calls across two list requests within TTL");
    }
}

mod list_changed_invalidation {
    //! `notifications/tools/list_changed` drops cache entries; next list re-checks SpiceDB.

    use super::*;

    #[tokio::test]
    async fn list_changed_notification_drops_session_zedtoken_entries() {
        let cache = ZedTokenCache::default();
        let params = CacheKeyParams::new("sess-inv", "trogon/principal:alice", "invoke");
        let resource = ResourceKey::new("trogon/mcp_tool", "server-a|tool");

        cache
            .insert_zed_token(&params, "tok-inv".into(), None)
            .await;
        cache
            .insert_results(&params, "tok-inv", &[(resource.clone(), true)], None)
            .await;

        cache.invalidate_server("server-a").await;

        assert!(
            cache
                .get_result(&params, &resource, Some("tok-inv"))
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn list_changed_for_other_server_does_not_invalidate_unrelated_session_cache() {
        let cache = ZedTokenCache::default();
        let params_a = CacheKeyParams::new("sess-a", "trogon/principal:alice", "invoke");
        let params_b = CacheKeyParams::new("sess-b", "trogon/principal:alice", "invoke");
        let resource_a = ResourceKey::new("trogon/mcp_tool", "server-a|tool");
        let resource_b = ResourceKey::new("trogon/mcp_tool", "server-b|tool");

        cache
            .insert_zed_token(&params_a, "tok-a".into(), None)
            .await;
        cache
            .insert_results(&params_a, "tok-a", &[(resource_a.clone(), true)], None)
            .await;
        cache
            .insert_zed_token(&params_b, "tok-b".into(), None)
            .await;
        cache
            .insert_results(&params_b, "tok-b", &[(resource_b.clone(), true)], None)
            .await;

        cache.invalidate_server("server-a").await;

        assert!(
            cache
                .get_result(&params_a, &resource_a, Some("tok-a"))
                .await
                .is_none()
        );
        assert_eq!(
            cache
                .get_result(&params_b, &resource_b, Some("tok-b"))
                .await,
            Some(true)
        );
    }

    #[tokio::test]
    #[ignore = "needs gateway notification subscriber wired to PermissionChecker::invalidate_tools_list_cache_for_server"]
    async fn tools_list_after_list_changed_reinvokes_bulk_check() {
        unimplemented!("assert cold PDP path after list_changed invalidation");
    }
}

mod ttl_expiry {
    //! Per-entry TTL eviction forces a fresh BulkCheck on the next `tools/list`.

    use super::*;

    #[tokio::test]
    async fn cache_entry_evicted_after_configured_ttl_elapses() {
        let cache = ZedTokenCache::new(ZedTokenCacheConfig {
            max_ttl: Duration::from_millis(1),
            ..ZedTokenCacheConfig::default()
        });
        let params = CacheKeyParams::new("sess-ttl", "trogon/principal:alice", "invoke");

        cache
            .insert_zed_token(&params, "tok-expire".into(), None)
            .await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(cache.get_zed_token(&params).await.is_none());
        assert!(cache.metrics().evictions_ttl.load(Ordering::Relaxed) >= 1);
    }

    #[tokio::test]
    async fn ttl_clamped_to_min_of_zed_expiration_and_configured_max() {
        let cfg = ZedTokenCacheConfig {
            max_ttl: Duration::from_secs(5),
            max_ttl_ceiling: Duration::from_secs(60),
            ..ZedTokenCacheConfig::default()
        };
        assert_eq!(cfg.entry_ttl(Some(Duration::from_secs(120))), Duration::from_secs(5));
        assert_eq!(cfg.entry_ttl(Some(Duration::from_secs(2))), Duration::from_secs(2));
    }

    #[tokio::test]
    #[ignore = "needs live NATS + SpiceDB mock to assert BulkCheck after TTL-driven cache miss"]
    async fn tools_list_after_ttl_expiry_reinvokes_bulk_check() {
        unimplemented!("assert cold PDP path after TTL safety net per ADR 0014");
    }
}

mod principal_isolation {
    //! Different `jwt.sub` values must not share ZedToken or permission result cache entries.

    use super::*;

    #[tokio::test]
    async fn different_jwt_sub_does_not_share_zedtoken_cache_entry() {
        let cache = ZedTokenCache::default();
        let params_a = CacheKeyParams::new("sess-shared", "trogon/principal:alice", "invoke");
        let params_b = CacheKeyParams::new("sess-shared", "trogon/principal:bob", "invoke");

        cache
            .insert_zed_token(&params_a, "tok-a".into(), None)
            .await;

        assert_eq!(cache.get_zed_token(&params_a).await.as_deref(), Some("tok-a"));
        assert!(cache.get_zed_token(&params_b).await.is_none());
    }

    #[tokio::test]
    async fn different_jwt_sub_does_not_hit_other_principals_permission_results() {
        let cache = ZedTokenCache::default();
        let params_a = CacheKeyParams::new("sess-shared", "trogon/principal:alice", "invoke");
        let params_b = CacheKeyParams::new("sess-shared", "trogon/principal:bob", "invoke");
        let resource = ResourceKey::new("trogon/mcp_tool", "filesystem|deploy");

        cache
            .insert_zed_token(&params_a, "tok-a".into(), None)
            .await;
        cache
            .insert_results(&params_a, "tok-a", &[(resource.clone(), true)], None)
            .await;

        assert_eq!(
            cache
                .get_result(&params_a, &resource, Some("tok-a"))
                .await,
            Some(true)
        );
        assert!(
            cache
                .get_result(&params_b, &resource, Some("tok-a"))
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn same_session_id_different_principal_gets_independent_cache_namespace() {
        let cache = ZedTokenCache::default();
        let params_a = CacheKeyParams::new("sess-shared", "trogon/principal:alice", "invoke");
        let params_b = CacheKeyParams::new("sess-shared", "trogon/principal:bob", "invoke");

        cache
            .insert_zed_token(&params_a, "tok-a".into(), None)
            .await;
        cache
            .insert_zed_token(&params_b, "tok-b".into(), None)
            .await;

        assert_eq!(cache.get_zed_token(&params_a).await.as_deref(), Some("tok-a"));
        assert_eq!(cache.get_zed_token(&params_b).await.as_deref(), Some("tok-b"));
        assert_ne!(
            ZedTokenCache::zed_key(&params_a),
            ZedTokenCache::zed_key(&params_b)
        );
    }
}
