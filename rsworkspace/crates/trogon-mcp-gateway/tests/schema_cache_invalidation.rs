//! Schema cache population, lookup, and invalidation integration tests.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use trogon_mcp_gateway::schema_cache::{
    SchemaCache, SchemaCacheConfig, SchemaCacheRuntime, SchemaSource, ServerId,
    handle_list_changed_notification, hash_schema, lookup_tool_schema, sniff_tools_list_reply,
    should_invalidate,
};

mod harness {
    use super::*;

    pub fn runtime_with_ttl(ttl: Duration) -> Arc<SchemaCacheRuntime> {
        SchemaCacheRuntime::new(SchemaCacheConfig {
            ttl,
            ..SchemaCacheConfig::default()
        })
    }

    pub fn shared_schema() -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"q": {"type": "string"}}})
    }

    pub fn tools_list_payload(tools: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "tools": tools }
        }))
        .expect("json")
    }
}

mod tools_list_populates_cache {
    use super::harness;
    use super::*;

    #[tokio::test]
    async fn tools_list_extracts_input_schemas_from_reply() {
        let runtime = harness::runtime_with_ttl(Duration::from_secs(120));
        let server = ServerId::new("fixture");
        let payload = harness::tools_list_payload(serde_json::json!([
            {
                "name": "alpha",
                "inputSchema": {"type": "object", "properties": {"x": {"type": "string"}}}
            },
            {
                "name": "beta",
                "inputSchema": {"type": "object", "properties": {"y": {"type": "integer"}}}
            }
        ]));

        let stored = sniff_tools_list_reply(&runtime.cache, &runtime.config, &server, &payload)
            .await
            .expect("sniff");
        assert_eq!(stored, 2);
        assert!(lookup_tool_schema(&runtime, &server, "alpha").await.expect("lookup").is_some());
        assert!(lookup_tool_schema(&runtime, &server, "beta").await.expect("lookup").is_some());
    }

    #[tokio::test]
    async fn tools_list_stores_schemas_keyed_by_server_id_and_hash() {
        let runtime = harness::runtime_with_ttl(Duration::from_secs(120));
        let server = ServerId::new("fixture");
        let shared = harness::shared_schema();
        let payload = harness::tools_list_payload(serde_json::json!([
            {"name": "tool_one", "inputSchema": shared},
            {"name": "tool_two", "inputSchema": {"type": "object", "properties": {"q": {"type": "string"}}}}
        ]));

        sniff_tools_list_reply(&runtime.cache, &runtime.config, &server, &payload)
            .await
            .expect("sniff");
        assert_eq!(runtime.cache.entry_count().await, 1);
    }

    #[tokio::test]
    async fn tools_list_records_schema_source_as_tools_list_sniff() {
        let runtime = harness::runtime_with_ttl(Duration::from_secs(120));
        let server = ServerId::new("fixture");
        let payload = harness::tools_list_payload(serde_json::json!([
            {"name": "alpha", "inputSchema": {"type": "object"}}
        ]));

        sniff_tools_list_reply(&runtime.cache, &runtime.config, &server, &payload)
            .await
            .expect("sniff");
        let cached = lookup_tool_schema(&runtime, &server, "alpha")
            .await
            .expect("lookup")
            .expect("cached");
        assert_eq!(cached.source, SchemaSource::ToolsListSniff);
    }
}

mod tools_call_cache_hit {
    use super::harness;
    use super::*;

    #[tokio::test]
    async fn tools_call_hits_cache_for_known_schema() {
        let runtime = harness::runtime_with_ttl(Duration::from_secs(120));
        let server = ServerId::new("fixture");
        let payload = harness::tools_list_payload(serde_json::json!([
            {"name": "run", "inputSchema": {"type": "object", "properties": {"cmd": {"type": "string"}}}}
        ]));
        sniff_tools_list_reply(&runtime.cache, &runtime.config, &server, &payload)
            .await
            .expect("warm");

        let hit = lookup_tool_schema(&runtime, &server, "run").await.expect("lookup");
        assert!(hit.is_some());
    }

    #[tokio::test]
    #[ignore = "needs NATS backend lane instrumentation for schema-fetch subject isolation"]
    async fn tools_call_does_not_fetch_schema_from_backend_on_hit() {
        unimplemented!("requires NATS harness counting backend tools/list refetch lane");
    }

    #[tokio::test]
    #[ignore = "needs NATS harness for cold-cache tools/call miss refetch path"]
    async fn tools_call_miss_fetches_and_populates_cache() {
        unimplemented!("wire ensure_tool_schema e2e with backend tools/list stub");
    }
}

mod list_changed_invalidation {
    use super::harness;
    use super::*;

    #[tokio::test]
    async fn list_changed_notification_invalidates_server_cache() {
        let runtime = harness::runtime_with_ttl(Duration::from_secs(120));
        let server = ServerId::new("fixture");
        runtime.record_client_server("desktop", &server);
        sniff_tools_list_reply(
            &runtime.cache,
            &runtime.config,
            &server,
            &harness::tools_list_payload(serde_json::json!([
                {"name": "alpha", "inputSchema": {"type": "object"}}
            ])),
        )
        .await
        .expect("warm");

        let invalidated = handle_list_changed_notification(
            &runtime,
            "mcp",
            "mcp.client.desktop.notifications.tools.list_changed",
            br#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#,
        )
        .await
        .expect("invalidate");
        assert!(invalidated);
        assert!(lookup_tool_schema(&runtime, &server, "alpha").await.expect("lookup").is_none());
    }

    #[tokio::test]
    async fn list_changed_on_client_notifications_subject_triggers_invalidation() {
        assert!(should_invalidate("notifications/tools/list_changed"));
        let runtime = harness::runtime_with_ttl(Duration::from_secs(120));
        let server = ServerId::new("fixture");
        runtime.record_client_server("agent-1", &server);
        sniff_tools_list_reply(
            &runtime.cache,
            &runtime.config,
            &server,
            &harness::tools_list_payload(serde_json::json!([
                {"name": "tool", "inputSchema": {"type": "object"}}
            ])),
        )
        .await
        .expect("warm");

        handle_list_changed_notification(
            &runtime,
            "mcp.test",
            "mcp.test.client.agent-1.notifications.tools.list_changed",
            br#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#,
        )
        .await
        .expect("handle");
        assert_eq!(runtime.cache.entry_count().await, 0);
    }

    #[tokio::test]
    #[ignore = "needs NATS harness counting backend schema-fetch after list_changed invalidation"]
    async fn next_tools_call_after_list_changed_refetches_schema() {
        unimplemented!("e2e tools/call refetch counter not wired in test harness");
    }
}

mod ttl_expiry {
    use super::harness;
    use super::*;
    use trogon_mcp_gateway::schema_cache::{CachedSchema, SchemaCache, SchemaCacheKey};

    #[tokio::test]
    async fn expired_cache_entry_is_evicted() {
        let runtime = harness::runtime_with_ttl(Duration::from_secs(1));
        let server = ServerId::new("fixture");
        let schema = serde_json::json!({"type": "object"});
        let key = SchemaCacheKey {
            server_id: server.clone(),
            schema_hash: hash_schema(&schema),
        };
        runtime
            .cache
            .put(
                key,
                CachedSchema {
                    schema,
                    fetched_at: SystemTime::now()
                        .checked_sub(Duration::from_secs(5))
                        .expect("clock"),
                    source: SchemaSource::ToolsListSniff,
                },
                "alpha",
            )
            .await
            .expect("put");
        assert!(lookup_tool_schema(&runtime, &server, "alpha").await.expect("lookup").is_none());
    }

    #[tokio::test]
    #[ignore = "needs NATS harness for tools/call refetch after TTL expiry"]
    async fn tools_call_after_ttl_refetches_schema() {
        unimplemented!("e2e TTL refetch requires backend tools/list stub");
    }

    #[tokio::test]
    async fn fresh_entry_not_evicted_before_ttl() {
        let runtime = harness::runtime_with_ttl(Duration::from_secs(120));
        let server = ServerId::new("fixture");
        sniff_tools_list_reply(
            &runtime.cache,
            &runtime.config,
            &server,
            &harness::tools_list_payload(serde_json::json!([
                {"name": "fresh", "inputSchema": {"type": "object"}}
            ])),
        )
        .await
        .expect("warm");
        assert!(lookup_tool_schema(&runtime, &server, "fresh").await.expect("lookup").is_some());
    }
}

mod reconnect_version_invalidation {
    use super::harness;
    use super::*;

    #[tokio::test]
    async fn server_reconnect_bumps_version_and_invalidates_cache() {
        let runtime = harness::runtime_with_ttl(Duration::from_secs(120));
        let server = ServerId::new("server-a");
        sniff_tools_list_reply(
            &runtime.cache,
            &runtime.config,
            &server,
            &harness::tools_list_payload(serde_json::json!([
                {"name": "tool", "inputSchema": {"type": "object"}}
            ])),
        )
        .await
        .expect("warm");
        assert_eq!(runtime.reconnect_generation(server.as_str()), 0);
        runtime.invalidate_on_reconnect(&server).await.expect("invalidate");
        assert_eq!(runtime.reconnect_generation(server.as_str()), 1);
        assert_eq!(runtime.cache.entry_count().await, 0);
    }

    #[tokio::test]
    #[ignore = "needs NATS harness for post-reconnect tools/call refetch without stale serve"]
    async fn stale_entries_after_reconnect_not_served() {
        unimplemented!("e2e reconnect stale-serve guard requires gateway + backend harness");
    }
}

mod cross_server_isolation {
    use super::harness;
    use super::*;

    async fn warm(server: &ServerId, runtime: &SchemaCacheRuntime, label: &str) {
        sniff_tools_list_reply(
            &runtime.cache,
            &runtime.config,
            server,
            &harness::tools_list_payload(serde_json::json!([
                {"name": label, "inputSchema": {"type": "object", "title": label}}
            ])),
        )
        .await
        .expect("warm");
    }

    #[tokio::test]
    async fn invalidating_server_a_does_not_evict_server_b() {
        let runtime = harness::runtime_with_ttl(Duration::from_secs(120));
        let server_a = ServerId::new("server-a");
        let server_b = ServerId::new("server-b");
        warm(&server_a, &runtime, "a").await;
        warm(&server_b, &runtime, "b").await;
        runtime.invalidate_server(&server_a).await.expect("invalidate a");
        assert!(lookup_tool_schema(&runtime, &server_a, "a").await.expect("lookup").is_none());
        assert!(lookup_tool_schema(&runtime, &server_b, "b").await.expect("lookup").is_some());
    }

    #[tokio::test]
    async fn cache_keys_are_scoped_by_server_id() {
        let runtime = harness::runtime_with_ttl(Duration::from_secs(120));
        let schema = harness::shared_schema();
        let server_a = ServerId::new("server-a");
        let server_b = ServerId::new("server-b");
        sniff_tools_list_reply(
            &runtime.cache,
            &runtime.config,
            &server_a,
            &harness::tools_list_payload(serde_json::json!([{"name": "shared", "inputSchema": schema.clone()}])),
        )
        .await
        .expect("warm a");
        sniff_tools_list_reply(
            &runtime.cache,
            &runtime.config,
            &server_b,
            &harness::tools_list_payload(serde_json::json!([{"name": "shared", "inputSchema": schema}])),
        )
        .await
        .expect("warm b");
        assert_eq!(runtime.cache.entry_count().await, 2);
        runtime.invalidate_server(&server_a).await.expect("invalidate a");
        assert_eq!(runtime.cache.entry_count().await, 1);
    }

    #[tokio::test]
    async fn reconnect_on_one_server_preserves_other_server_cache() {
        let runtime = harness::runtime_with_ttl(Duration::from_secs(120));
        let server_a = ServerId::new("server-a");
        let server_b = ServerId::new("server-b");
        warm(&server_a, &runtime, "a").await;
        warm(&server_b, &runtime, "b").await;
        runtime.invalidate_on_reconnect(&server_a).await.expect("reconnect a");
        assert!(lookup_tool_schema(&runtime, &server_b, "b").await.expect("lookup").is_some());
    }
}

mod concurrent_singleflight {
    //! Covered by unit tests in `schema_cache/singleflight.rs`; integration e2e deferred.

    #[tokio::test]
    #[ignore = "singleflight dedupe covered in schema_cache/singleflight.rs unit tests; NATS e2e deferred"]
    async fn concurrent_uncached_fetches_deduplicate_to_single_backend_call() {
        unimplemented!("see schema_cache::singleflight unit tests");
    }

    #[tokio::test]
    #[ignore = "singleflight waiters covered in schema_cache/singleflight.rs unit tests; NATS e2e deferred"]
    async fn singleflight_waiters_receive_same_cached_result() {
        unimplemented!("see schema_cache::singleflight unit tests");
    }

    #[tokio::test]
    #[ignore = "singleflight parallel keys covered in schema_cache/singleflight.rs unit tests; NATS e2e deferred"]
    async fn singleflight_does_not_block_unrelated_schema_fetches() {
        unimplemented!("see schema_cache::singleflight unit tests");
    }
}

mod gateway_e2e {
    use std::sync::Arc;
    use std::time::Duration;

    use bytes::Bytes;
    use futures::StreamExt;
    use mcp_nats::{Config as McpConfig, McpPrefix};
    use trogon_mcp_gateway::authz::AllowAllPermissionChecker;
    use trogon_mcp_gateway::gateway::GatewaySettings;
    use trogon_mcp_gateway::schema_cache::{SchemaCacheConfig, SchemaCacheRuntime, lookup_tool_schema};
    use trogon_nats::{NatsAuth, NatsConfig, connect};
    use uuid::Uuid;

    #[tokio::test]
    #[ignore = "needs NATS (set NATS_URL). Run: cargo test -p trogon-mcp-gateway --test schema_cache_invalidation -- --ignored"]
    async fn gateway_sniff_populates_shared_schema_cache_on_tools_list() {
        let url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
        let nats_conf = NatsConfig::new(vec![url], NatsAuth::None);
        let prefix_segment = format!("{}", Uuid::now_v7().as_simple());
        let prefix_token = format!("{}.mcp", prefix_segment);
        let prefix = McpPrefix::new(&prefix_token).expect("prefix");
        let mcp_conf = McpConfig::new(prefix.clone(), nats_conf.clone()).with_operation_timeout(Duration::from_secs(15));

        let backend = connect(&nats_conf, Duration::from_secs(15)).await.expect("backend nats");
        let tools = serde_json::json!([
            {"name": "echo", "inputSchema": {"type": "object", "properties": {"msg": {"type": "string"}}}}
        ]);
        let backend_reply = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"tools": tools}
        }))
        .expect("json");

        let subject_out = format!("{}.server.fixture.tools.list", prefix.as_str());
        {
            let mut sub = backend.subscribe(subject_out.clone()).await.expect("sub");
            let backend_nats = backend.clone();
            tokio::spawn(async move {
                while let Some(msg) = sub.next().await {
                    let Some(reply) = msg.reply else { continue };
                    backend_nats
                        .publish_with_headers(reply.to_string(), async_nats::HeaderMap::new(), Bytes::from(backend_reply))
                        .await
                        .ok();
                    backend_nats.flush().await.ok();
                    break;
                }
            });
        }

        let runtime = SchemaCacheRuntime::new(SchemaCacheConfig::default());
        let _ = SchemaCacheRuntime::install(runtime.clone());

        let settings = GatewaySettings {
            queue_group: format!("q-{prefix_segment}"),
            audit_stream_name: "MCP_AUDIT_SCHEMA_CACHE".into(),
            init_audit_stream: false,
            mcp: mcp_conf,
            jwt: trogon_mcp_gateway::jwt::JwtValidator::disabled().expect("jwt off"),
            egress: None,
            chain_resolver: None,
            rate_limit: None,
            approval_gate: None,
            mesh_config: trogon_mcp_gateway::policy::MeshGatewayConfig::default(),
            context_throttle: None,
            anomaly_emitter: None,
        };

        let gateway_client = Arc::new(connect(&nats_conf, Duration::from_secs(15)).await.expect("gateway nats"));
        let checker: Arc<dyn trogon_mcp_gateway::authz::PermissionChecker> = Arc::new(AllowAllPermissionChecker);
        let traces = trogon_mcp_gateway::trace::TraceStore::default();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let run_client = gateway_client.clone();
        let join = tokio::spawn(async move {
            trogon_mcp_gateway::run(run_client, checker, traces, settings, async {
                shutdown_rx.await.ok();
            })
            .await
        });

        tokio::time::sleep(Duration::from_millis(400)).await;

        let mut headers = async_nats::HeaderMap::new();
        headers.insert("mcp-client-id", "desktop");
        let ingress = format!("{}.gateway.request.fixture.tools.list", prefix.as_str());
        let payload = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}});
        gateway_client
            .request_with_headers(
                ingress,
                headers,
                serde_json::to_vec(&payload).unwrap().into(),
            )
            .await
            .expect("tools/list");

        let server = trogon_mcp_gateway::schema_cache::ServerId::new("fixture");
        let cached = lookup_tool_schema(&runtime, &server, "echo").await.expect("lookup");
        assert!(cached.is_some(), "gateway sniff should populate shared cache");

        shutdown_tx.send(()).ok();
        join.await.expect("join").expect("run");
    }
}
