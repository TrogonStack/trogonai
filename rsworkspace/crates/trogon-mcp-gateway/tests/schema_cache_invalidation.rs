//! Schema cache population, lookup, and invalidation integration tests (scaffold).
//!
//! Phase 2 caches JSON schemas extracted from backend MCP `tools/list` replies, keyed by
//! `(server_id, schema_hash)`. Subsequent `tools/call` requests should hit the cache without
//! consulting the backend for schema fetch. Invalidation triggers include
//! `notifications/tools/list_changed` on `mcp.client.{client_id}.notifications.>`, TTL expiry,
//! and server reconnect version bumps.
//!
//! Cross-refs:
//! - `MCP_GATEWAY_PLAN.md` Block C item 3 and section "10. What Agentgateway Has That We Will Need to Build"
//! - `trogon_mcp_gateway::schema_cache` (`InMemorySchemaCache`, `SchemaCacheKey`, `ServerId`,
//!   `SchemaHash`, `should_invalidate`)
//!
//! Harness pattern: `mcp_nats::Config`, `McpPrefix`, `trogon_nats::NatsAuth`, `GatewaySettings`,
//! `AllowAllPermissionChecker`, and `trogon_mcp_gateway::run` (see `e2e_nats_forward.rs`).
//!
//! Once schema cache wiring lands, remove `#[ignore]` on each test and fill in Arrange / Act / Assert.

mod tools_list_populates_cache {
    //! First `tools/list` reply extracts `tools[].inputSchema` and stores entries keyed by
    //! `(server_id, schema_hash)`.

    #[tokio::test]
    #[ignore = "scaffold; implement when schema cache invalidation per MCP_GATEWAY_PLAN.md Block C item 3 lands"]
    async fn tools_list_extracts_input_schemas_from_reply() {
        // Arrange: stand up gateway + backend stub replying to `{prefix}.server.{server_id}.tools.list`
        //          with tools carrying distinct `inputSchema` payloads.
        // Act: client sends `tools/list` via `{prefix}.gateway.request.{server_id}.tools.list`.
        // Assert: `SchemaCache::get` returns `CachedSchema` for each `(server_id, schema_hash)`.
        unimplemented!("extract inputSchema from tools/list result");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema cache invalidation per MCP_GATEWAY_PLAN.md Block C item 3 lands"]
    async fn tools_list_stores_schemas_keyed_by_server_id_and_hash() {
        // Arrange: seed backend with two tools on the same server sharing one schema hash.
        // Act: single `tools/list` through gateway.
        // Assert: cache holds one entry per unique `(ServerId, SchemaHash)` pair, not per tool name.
        unimplemented!("cache key is (server_id, schema_hash)");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema cache invalidation per MCP_GATEWAY_PLAN.md Block C item 3 lands"]
    async fn tools_list_records_schema_source_as_tools_list_sniff() {
        // Arrange: gateway with empty `InMemorySchemaCache`, backend returns one tool schema.
        // Act: `tools/list`.
        // Assert: stored `CachedSchema.source` is `SchemaSource::ToolsListSniff`.
        unimplemented!("schema source provenance on first sniff");
    }
}

mod tools_call_cache_hit {
    //! Second `tools/call` for the same tool hits cache; backend is not consulted for schema fetch.

    #[tokio::test]
    #[ignore = "scaffold; implement when schema cache invalidation per MCP_GATEWAY_PLAN.md Block C item 3 lands"]
    async fn tools_call_hits_cache_for_known_schema() {
        // Arrange: warm cache via prior `tools/list`; instrument backend schema-fetch lane.
        // Act: `tools/call` with arguments matching cached tool.
        // Assert: cache `get` succeeds; no backend schema-fetch request observed.
        unimplemented!("tools/call uses cached schema on hit");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema cache invalidation per MCP_GATEWAY_PLAN.md Block C item 3 lands"]
    async fn tools_call_does_not_fetch_schema_from_backend_on_hit() {
        // Arrange: subscribe to any backend subject used for explicit schema retrieval; warm cache.
        // Act: second `tools/call` for same `(server_id, tool)`.
        // Assert: backend schema-fetch subscription receives zero messages.
        unimplemented!("no backend round-trip when cache populated");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema cache invalidation per MCP_GATEWAY_PLAN.md Block C item 3 lands"]
    async fn tools_call_miss_fetches_and_populates_cache() {
        // Arrange: cold cache, backend stub for both `tools/call` and schema fetch.
        // Act: first `tools/call` without prior `tools/list`.
        // Assert: cache populated after miss; subsequent call hits cache.
        unimplemented!("cache miss path refetches then stores");
    }
}

mod list_changed_invalidation {
    //! `notifications/tools/list_changed` on `mcp.client.{client_id}.notifications.>` invalidates
    //! that server's cache; the next call refetches.

    #[tokio::test]
    #[ignore = "scaffold; implement when schema cache invalidation per MCP_GATEWAY_PLAN.md Block C item 3 lands"]
    async fn list_changed_notification_invalidates_server_cache() {
        // Arrange: warm cache for `server_id`; publish MCP notification with method
        //          `notifications/tools/list_changed` (see `should_invalidate`).
        // Act: gateway receives notification on client notifications subject.
        // Assert: `SchemaCache::get` returns None for all keys under that `ServerId`.
        unimplemented!("list_changed drops cached schemas for server");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema cache invalidation per MCP_GATEWAY_PLAN.md Block C item 3 lands"]
    async fn list_changed_on_client_notifications_subject_triggers_invalidation() {
        // Arrange: gateway subscribed to `mcp.client.{client_id}.notifications.>`.
        // Act: publish `notifications/tools/list_changed` to matching subject.
        // Assert: invalidation handler runs for the notification's server scope.
        unimplemented!("notification subject wiring per MCP_GATEWAY_PLAN.md Block C item 3");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema cache invalidation per MCP_GATEWAY_PLAN.md Block C item 3 lands"]
    async fn next_tools_call_after_list_changed_refetches_schema() {
        // Arrange: warm cache, send `list_changed`, backend stub counts schema-fetch calls.
        // Act: `tools/call` after invalidation.
        // Assert: exactly one backend schema fetch; cache repopulated.
        unimplemented!("post-invalidation refetch on next call");
    }
}

mod ttl_expiry {
    //! TTL expiry evicts cache entries; the next call refetches.

    #[tokio::test]
    #[ignore = "scaffold; implement when schema cache invalidation per MCP_GATEWAY_PLAN.md Block C item 3 lands"]
    async fn expired_cache_entry_is_evicted() {
        // Arrange: populate cache with entry whose `fetched_at` + TTL is in the past.
        // Act: advance clock or wait for TTL (per gateway config).
        // Assert: `SchemaCache::get` returns None.
        unimplemented!("TTL eviction of stale entries");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema cache invalidation per MCP_GATEWAY_PLAN.md Block C item 3 lands"]
    async fn tools_call_after_ttl_refetches_schema() {
        // Arrange: warm cache, elapse TTL, instrument backend schema-fetch lane.
        // Act: `tools/call`.
        // Assert: backend consulted once; cache repopulated with fresh `fetched_at`.
        unimplemented!("refetch after TTL expiry");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema cache invalidation per MCP_GATEWAY_PLAN.md Block C item 3 lands"]
    async fn fresh_entry_not_evicted_before_ttl() {
        // Arrange: populate cache; TTL not yet elapsed.
        // Act: `tools/call`.
        // Assert: cache hit; no backend schema fetch.
        unimplemented!("entries survive until TTL");
    }
}

mod reconnect_version_invalidation {
    //! Server reconnect bumps version and invalidates that server's cache entries.

    #[tokio::test]
    #[ignore = "scaffold; implement when schema cache invalidation per MCP_GATEWAY_PLAN.md Block C item 3 lands"]
    async fn server_reconnect_bumps_version_and_invalidates_cache() {
        // Arrange: warm cache for `server_id`; simulate backend disconnect + reconnect.
        // Act: gateway observes reconnect / version bump for that server.
        // Assert: all `(server_id, _)` cache keys removed.
        unimplemented!("reconnect version bump invalidates server cache");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema cache invalidation per MCP_GATEWAY_PLAN.md Block C item 3 lands"]
    async fn stale_entries_after_reconnect_not_served() {
        // Arrange: warm cache, force reconnect without new `tools/list`.
        // Act: `tools/call`.
        // Assert: stale pre-reconnect schema not used; refetch occurs.
        unimplemented!("no stale schema served after reconnect");
    }
}

mod cross_server_isolation {
    //! Invalidating server A does not evict server B entries.

    #[tokio::test]
    #[ignore = "scaffold; implement when schema cache invalidation per MCP_GATEWAY_PLAN.md Block C item 3 lands"]
    async fn invalidating_server_a_does_not_evict_server_b() {
        // Arrange: warm cache for `server-a` and `server-b`.
        // Act: `list_changed` or `invalidate` scoped to `server-a` only.
        // Assert: `server-b` entries remain; `server-a` entries gone.
        unimplemented!("cross-server cache isolation on invalidation");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema cache invalidation per MCP_GATEWAY_PLAN.md Block C item 3 lands"]
    async fn cache_keys_are_scoped_by_server_id() {
        // Arrange: identical schema hash on two servers.
        // Act: populate both via separate `tools/list` replies.
        // Assert: two distinct `SchemaCacheKey` entries; invalidating one leaves the other.
        unimplemented!("(server_id, schema_hash) key scoping");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema cache invalidation per MCP_GATEWAY_PLAN.md Block C item 3 lands"]
    async fn reconnect_on_one_server_preserves_other_server_cache() {
        // Arrange: warm cache for `server-a` and `server-b`.
        // Act: reconnect only `server-a`.
        // Assert: `server-b` cache entries unchanged and still hit on `tools/call`.
        unimplemented!("reconnect isolation across servers");
    }
}

mod concurrent_singleflight {
    //! Concurrent fetches for the same uncached schema deduplicate (singleflight).

    #[tokio::test]
    #[ignore = "scaffold; implement when schema cache invalidation per MCP_GATEWAY_PLAN.md Block C item 3 lands"]
    async fn concurrent_uncached_fetches_deduplicate_to_single_backend_call() {
        // Arrange: cold cache; N concurrent `tools/call` for same `(server_id, schema_hash)`.
        // Act: all calls in flight before first completes.
        // Assert: backend schema-fetch lane receives exactly one request.
        unimplemented!("singleflight deduplicates concurrent misses");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema cache invalidation per MCP_GATEWAY_PLAN.md Block C item 3 lands"]
    async fn singleflight_waiters_receive_same_cached_result() {
        // Arrange: cold cache; spawn concurrent callers awaiting same schema.
        // Act: first fetch completes.
        // Assert: all waiters succeed with identical validated outcome; cache holds one entry.
        unimplemented!("waiters share singleflight result");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema cache invalidation per MCP_GATEWAY_PLAN.md Block C item 3 lands"]
    async fn singleflight_does_not_block_unrelated_schema_fetches() {
        // Arrange: cold cache; concurrent calls for different `(server_id, schema_hash)` pairs.
        // Act: parallel `tools/call` across distinct keys.
        // Assert: one backend fetch per distinct key; no cross-key blocking.
        unimplemented!("singleflight keyed per cache entry");
    }
}
