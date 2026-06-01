//! Integration tests for per-server / per-tenant inflight caps and per-caller rate budgets.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use mcp_nats::{Config as McpConfig, McpPrefix};
use serde_json::{Value, json};
use trogon_mcp_gateway::authz::AllowAllPermissionChecker;
use trogon_mcp_gateway::gateway::GatewaySettings;
use trogon_mcp_gateway::rpc_codes;
use trogon_mcp_gateway::throttle::{RateLimitConfig, RateLimiter};
use trogon_nats::{NatsAuth, NatsConfig, connect};
use uuid::Uuid;

const RATE_LIMITED: i32 = rpc_codes::RATE_LIMITED;

mod harness {
    use super::*;

    pub const TEST_SERVER_INFLIGHT_CAP: u32 = 2;
    pub const TEST_TENANT_INFLIGHT_CAP: u32 = 2;
    pub const TEST_CALLER_BUDGET: u32 = 3;
    pub const TEST_CALLER_WINDOW: Duration = Duration::from_secs(1);
    pub const BACKEND_HOLD: Duration = Duration::from_secs(5);

    const HS_SECRET: &[u8] = b"rate-limit-test-secret-at-least-32-bytes!";
    const HS_ISSUER: &str = "https://rate-limit.test/";
    const GATEWAY_AUD: &str = "urn:trogon:mcp:gateway:acme:rate-test";

    pub fn test_rate_limiter() -> Arc<RateLimiter> {
        Arc::new(RateLimiter::new(RateLimitConfig {
            enabled: true,
            server_inflight_max: TEST_SERVER_INFLIGHT_CAP,
            tenant_inflight_max: TEST_TENANT_INFLIGHT_CAP,
            caller_budget: TEST_CALLER_BUDGET,
            caller_window: TEST_CALLER_WINDOW,
        }))
    }

    pub fn jwt_for_sub(sub: &str, tenant: &str) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let claims = json!({
            "iss": HS_ISSUER,
            "sub": sub,
            "aud": GATEWAY_AUD,
            "exp": now + 3600,
            "iat": now,
            "tenant": tenant,
            "https://trogon.ai/tenant": tenant,
        });
        encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(HS_SECRET),
        )
        .expect("sign jwt")
    }

    pub fn jwt_validator() -> Arc<trogon_mcp_gateway::jwt::JwtValidator> {
        trogon_mcp_gateway::jwt::JwtValidator::try_new(trogon_mcp_gateway::jwt::JwtIngressConfig {
            mode: trogon_mcp_gateway::jwt::JwtMode::Require,
            agent_identity_mode: trogon_mcp_gateway::agent_identity::AgentIdentityMode::Off,
            issuers: [HS_ISSUER.to_string()].into(),
            trusted_mint_issuers: [HS_ISSUER.to_string()].into(),
            audience: GATEWAY_AUD.into(),
            leeway_secs: 60,
            tenant_claim_key: "https://trogon.ai/tenant".into(),
            bearer_header_name: "authorization".into(),
            hs256_secret: Some(HS_SECRET.to_vec()),
            rsa_public_key_pem: None,
            jwks_uri: None,
        })
        .expect("jwt validator")
    }

    pub struct GatewayHarness {
        pub nats: Arc<async_nats::Client>,
        pub prefix: McpPrefix,
        pub server_id: String,
        pub shutdown_tx: tokio::sync::oneshot::Sender<()>,
        pub join: tokio::task::JoinHandle<Result<(), trogon_mcp_gateway::GatewayError>>,
    }

    pub async fn start_gateway(
        rate_limit: Arc<RateLimiter>,
        init_audit_stream: bool,
    ) -> GatewayHarness {
        let url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
        let connect_timeout = Duration::from_secs(15);
        let nats_conf = NatsConfig::new(vec![url], NatsAuth::None);
        let prefix_segment = format!("{}", Uuid::now_v7().as_simple());
        let prefix = McpPrefix::new(format!("{prefix_segment}.mcp")).expect("prefix");
        let mcp_conf =
            McpConfig::new(prefix.clone(), nats_conf.clone()).with_operation_timeout(Duration::from_secs(15));
        let server_id = "fixture".to_string();

        let settings = GatewaySettings {
            queue_group: format!("q-{prefix_segment}"),
            audit_stream_name: format!("MCP_AUDIT_RL_{prefix_segment}"),
            init_audit_stream,
            mcp: mcp_conf,
            jwt: jwt_validator(),
            egress: None,
            chain_resolver: None,
            rate_limit: Some(rate_limit),
            context_throttle: None,
        };

        let nats = Arc::new(connect(&nats_conf, connect_timeout).await.expect("nats connect"));
        let checker: Arc<dyn trogon_mcp_gateway::authz::PermissionChecker> =
            Arc::new(AllowAllPermissionChecker);
        let traces = trogon_mcp_gateway::trace::TraceStore::default();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let run_nats = nats.clone();
        let join = tokio::spawn(async move {
            trogon_mcp_gateway::run(run_nats, checker, traces, settings, async {
                shutdown_rx.await.ok();
            })
            .await
        });
        tokio::time::sleep(Duration::from_millis(350)).await;
        GatewayHarness {
            nats,
            prefix,
            server_id,
            shutdown_tx,
            join,
        }
    }

    pub async fn spawn_slow_backend(
        nats: Arc<async_nats::Client>,
        subject_out: String,
        hold: Duration,
    ) {
        tokio::spawn(async move {
            let mut sub = nats.subscribe(subject_out).await.expect("backend sub");
            while let Some(msg) = sub.next().await {
                tokio::time::sleep(hold).await;
                if let Some(reply) = msg.reply {
                    let body = json!({"jsonrpc":"2.0","id":1,"result":{"tools":[]}});
                    nats.publish(reply.to_string(), serde_json::to_vec(&body).unwrap().into())
                        .await
                        .ok();
                    nats.flush().await.ok();
                }
            }
        });
    }

    pub async fn spawn_fast_backend(nats: Arc<async_nats::Client>, subject_out: String) {
        tokio::spawn(async move {
            let mut sub = nats.subscribe(subject_out).await.expect("backend sub");
            while let Some(msg) = sub.next().await {
                if let Some(reply) = msg.reply {
                    let body = json!({"jsonrpc":"2.0","id":1,"result":{"tools":[]}});
                    nats.publish(reply.to_string(), serde_json::to_vec(&body).unwrap().into())
                        .await
                        .ok();
                    nats.flush().await.ok();
                }
            }
        });
    }

    pub async fn tools_list(
        nats: &async_nats::Client,
        prefix: &McpPrefix,
        server_id: &str,
        sub: &str,
        tenant: &str,
        request_id: i64,
    ) -> async_nats::Message {
        let ingress = format!("{}.gateway.request.{server_id}.tools.list", prefix.as_str());
        let token = jwt_for_sub(sub, tenant);
        let mut headers = async_nats::HeaderMap::new();
        headers.insert("authorization", format!("Bearer {token}").as_str());
        let payload = json!({"jsonrpc":"2.0","id":request_id,"method":"tools/list","params":{}});
        nats.request_with_headers(
            ingress,
            headers,
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("ingress request")
    }

    pub fn assert_rate_limited(body: &Value, scope: &str) -> u64 {
        let err = body.get("error").expect("error object");
        assert_eq!(err.get("code").and_then(|v| v.as_i64()), Some(RATE_LIMITED as i64));
        assert_eq!(err.get("message").and_then(|v| v.as_str()), Some("rate_limited"));
        let data = err.get("data").expect("error.data");
        assert_eq!(data.get("scope").and_then(|v| v.as_str()), Some(scope));
        let retry = data
            .get("retry_after_ms")
            .and_then(|v| v.as_u64())
            .expect("retry_after_ms");
        assert!(retry > 0);
        retry
    }
}

mod unit {
    use super::harness::{TEST_CALLER_BUDGET, TEST_CALLER_WINDOW, test_rate_limiter};
    use super::Duration;
    use trogon_mcp_gateway::throttle::RateLimitScope;

    #[test]
    fn caller_budget_blocks_fourth_request() {
        let limiter = test_rate_limiter();
        for _ in 0..TEST_CALLER_BUDGET {
            assert!(limiter.check_caller("acme", "user:alice").is_none());
        }
        let deny = limiter.check_caller("acme", "user:alice").expect("deny");
        assert_eq!(deny.scope, RateLimitScope::Caller);
        assert!(deny.retry_after_ms > 0);
    }

    #[test]
    fn distinct_subs_isolated() {
        let limiter = test_rate_limiter();
        for _ in 0..TEST_CALLER_BUDGET {
            limiter.check_caller("acme", "user:alice");
        }
        assert!(limiter.check_caller("acme", "user:alice").is_some());
        assert!(limiter.check_caller("acme", "user:bob").is_none());
    }

    #[test]
    fn inflight_releases_after_guard_drop() {
        let limiter = test_rate_limiter();
        let g1 = limiter.try_acquire_inflight("srv", "acme").unwrap();
        let _g2 = limiter.try_acquire_inflight("srv", "acme").unwrap();
        assert!(limiter.try_acquire_inflight("srv", "acme").is_err());
        drop(g1);
        assert!(limiter.try_acquire_inflight("srv", "acme").is_ok());
    }

    #[test]
    fn caller_budget_refills_after_window() {
        let limiter = test_rate_limiter();
        for _ in 0..TEST_CALLER_BUDGET {
            limiter.check_caller("acme", "user:alice");
        }
        assert!(limiter.check_caller("acme", "user:alice").is_some());
        std::thread::sleep(TEST_CALLER_WINDOW + Duration::from_millis(50));
        assert!(limiter.check_caller("acme", "user:alice").is_none());
    }
}

mod per_server_inflight {
    use super::harness::{
        BACKEND_HOLD, assert_rate_limited, spawn_fast_backend, spawn_slow_backend, start_gateway,
        test_rate_limiter, tools_list,
    };
    use super::{Duration, Value};

    #[tokio::test]
    #[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test rate_limit_caps -- --ignored"]
    async fn third_concurrent_request_returns_rate_limited_server_scope() {
        let h = start_gateway(test_rate_limiter(), false).await;
        let subject = format!("{}.server.{}.tools.list", h.prefix.as_str(), h.server_id);
        spawn_slow_backend(h.nats.clone(), subject, BACKEND_HOLD).await;

        let nats = h.nats.clone();
        let prefix = h.prefix.clone();
        let server_id = h.server_id.clone();
        let f1 = tokio::spawn(async move {
            tools_list(&nats, &prefix, &server_id, "user:a", "acme", 1).await
        });
        let nats = h.nats.clone();
        let prefix = h.prefix.clone();
        let server_id = h.server_id.clone();
        let f2 = tokio::spawn(async move {
            tools_list(&nats, &prefix, &server_id, "user:b", "acme", 2).await
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        let third = tools_list(&h.nats, &h.prefix, &h.server_id, "user:c", "acme", 3).await;
        let body: Value = serde_json::from_slice(&third.payload).expect("json");
        assert_rate_limited(&body, "server");
        let _ = f1.await;
        let _ = f2.await;
        h.shutdown_tx.send(()).ok();
        h.join.await.expect("join").expect("gateway run");
    }

    #[tokio::test]
    #[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test rate_limit_caps -- --ignored"]
    async fn server_inflight_cap_releases_slot_on_backend_completion() {
        let h = start_gateway(test_rate_limiter(), false).await;
        let subject = format!("{}.server.{}.tools.list", h.prefix.as_str(), h.server_id);
        spawn_slow_backend(h.nats.clone(), subject, Duration::from_millis(300)).await;

        let nats = h.nats.clone();
        let prefix = h.prefix.clone();
        let server_id = h.server_id.clone();
        let slow1 = tokio::spawn(async move {
            tools_list(&nats, &prefix, &server_id, "user:a", "acme", 1).await
        });
        let nats = h.nats.clone();
        let prefix = h.prefix.clone();
        let server_id = h.server_id.clone();
        let slow2 = tokio::spawn(async move {
            tools_list(&nats, &prefix, &server_id, "user:b", "acme", 2).await
        });
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(tools_list(&h.nats, &h.prefix, &h.server_id, "user:c", "acme", 3)
            .await
            .payload
            .starts_with(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"error\""));

        let _ = slow1.await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        let third = tools_list(&h.nats, &h.prefix, &h.server_id, "user:d", "acme", 4).await;
        let body: Value = serde_json::from_slice(&third.payload).expect("json");
        assert!(body.get("result").is_some(), "expected success after slot release, got {body}");
        let _ = slow2.await;
        h.shutdown_tx.send(()).ok();
        h.join.await.expect("join").expect("gateway run");
    }

    #[tokio::test]
    #[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test rate_limit_caps -- --ignored"]
    async fn under_cap_requests_succeed_without_rate_limit() {
        let h = start_gateway(test_rate_limiter(), false).await;
        let subject = format!("{}.server.{}.tools.list", h.prefix.as_str(), h.server_id);
        spawn_fast_backend(h.nats.clone(), subject).await;

        for id in 1..=2 {
            let resp = tools_list(&h.nats, &h.prefix, &h.server_id, "user:a", "acme", id).await;
            let body: Value = serde_json::from_slice(&resp.payload).expect("json");
            assert!(body.get("result").is_some(), "request {id} should succeed");
        }
        h.shutdown_tx.send(()).ok();
        h.join.await.expect("join").expect("gateway run");
    }
}

mod per_tenant_inflight {
    use super::harness::{
        BACKEND_HOLD, assert_rate_limited, spawn_slow_backend, start_gateway, test_rate_limiter,
        tools_list,
    };
    use super::{Duration, Value};

    #[tokio::test]
    #[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test rate_limit_caps -- --ignored"]
    async fn third_concurrent_request_returns_rate_limited_tenant_scope() {
        let h = start_gateway(test_rate_limiter(), false).await;
        for server in ["srv-a", "srv-b", "srv-c"] {
            let subject = format!("{}.server.{server}.tools.list", h.prefix.as_str());
            spawn_slow_backend(h.nats.clone(), subject, BACKEND_HOLD).await;
        }

        let nats = h.nats.clone();
        let prefix = h.prefix.clone();
        tokio::spawn(async move {
            tools_list(&nats, &prefix, "srv-a", "user:a", "acme", 1).await
        });
        let nats = h.nats.clone();
        let prefix = h.prefix.clone();
        tokio::spawn(async move {
            tools_list(&nats, &prefix, "srv-b", "user:b", "acme", 2).await
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        let third = tools_list(&h.nats, &h.prefix, "srv-c", "user:c", "acme", 3).await;
        let body: Value = serde_json::from_slice(&third.payload).expect("json");
        assert_rate_limited(&body, "tenant");
        h.shutdown_tx.send(()).ok();
        h.join.await.expect("join").expect("gateway run");
    }

    #[tokio::test]
    #[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test rate_limit_caps -- --ignored"]
    async fn tenant_inflight_counts_across_server_ids() {
        let h = start_gateway(test_rate_limiter(), false).await;
        for server in ["alpha", "beta", "gamma"] {
            let subject = format!("{}.server.{server}.tools.list", h.prefix.as_str());
            spawn_slow_backend(h.nats.clone(), subject, BACKEND_HOLD).await;
        }
        let nats = h.nats.clone();
        let prefix = h.prefix.clone();
        tokio::spawn(async move { tools_list(&nats, &prefix, "alpha", "u1", "acme", 1).await });
        let nats = h.nats.clone();
        let prefix = h.prefix.clone();
        tokio::spawn(async move { tools_list(&nats, &prefix, "beta", "u2", "acme", 2).await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        let body: Value =
            serde_json::from_slice(&tools_list(&h.nats, &h.prefix, "gamma", "u3", "acme", 3).await.payload)
                .unwrap();
        assert_rate_limited(&body, "tenant");
        h.shutdown_tx.send(()).ok();
        h.join.await.expect("join").expect("gateway run");
    }

    #[tokio::test]
    #[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test rate_limit_caps -- --ignored"]
    async fn distinct_tenants_do_not_share_inflight_budget() {
        let h = start_gateway(test_rate_limiter(), false).await;
        for (server, tenant) in [("s1", "tenant_a"), ("s2", "tenant_a"), ("s3", "tenant_b")] {
            let subject = format!("{}.server.{server}.tools.list", h.prefix.as_str());
            spawn_slow_backend(h.nats.clone(), subject, BACKEND_HOLD).await;
            let _ = (server, tenant);
        }
        let nats = h.nats.clone();
        let prefix = h.prefix.clone();
        tokio::spawn(async move { tools_list(&nats, &prefix, "s1", "a", "tenant_a", 1).await });
        let nats = h.nats.clone();
        let prefix = h.prefix.clone();
        tokio::spawn(async move { tools_list(&nats, &prefix, "s2", "b", "tenant_a", 2).await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        let resp = tools_list(&h.nats, &h.prefix, "s3", "c", "tenant_b", 3).await;
        let body: Value = serde_json::from_slice(&resp.payload).expect("json");
        assert!(body.get("result").is_some(), "tenant_b should succeed: {body}");
        h.shutdown_tx.send(()).ok();
        h.join.await.expect("join").expect("gateway run");
    }
}

mod per_caller_rate_budget {
    use super::harness::{
        TEST_CALLER_BUDGET, assert_rate_limited, spawn_fast_backend, start_gateway, test_rate_limiter,
        tools_list,
    };
    use super::Value;

    #[tokio::test]
    #[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test rate_limit_caps -- --ignored"]
    async fn fourth_request_in_one_second_returns_caller_scope() {
        let h = start_gateway(test_rate_limiter(), false).await;
        let subject = format!("{}.server.{}.tools.list", h.prefix.as_str(), h.server_id);
        spawn_fast_backend(h.nats.clone(), subject).await;

        for id in 1..=TEST_CALLER_BUDGET {
            let resp = tools_list(&h.nats, &h.prefix, &h.server_id, "user:alice", "acme", id as i64).await;
            let body: Value = serde_json::from_slice(&resp.payload).expect("json");
            assert!(body.get("result").is_some(), "request {id} should succeed");
        }
        let fourth = tools_list(&h.nats, &h.prefix, &h.server_id, "user:alice", "acme", 4).await;
        let body: Value = serde_json::from_slice(&fourth.payload).expect("json");
        assert_rate_limited(&body, "caller");
        h.shutdown_tx.send(()).ok();
        h.join.await.expect("join").expect("gateway run");
    }

    #[tokio::test]
    #[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test rate_limit_caps -- --ignored"]
    async fn rate_limited_response_includes_positive_retry_after_ms() {
        let h = start_gateway(test_rate_limiter(), false).await;
        let subject = format!("{}.server.{}.tools.list", h.prefix.as_str(), h.server_id);
        spawn_fast_backend(h.nats.clone(), subject).await;
        for id in 1..=4 {
            let resp = tools_list(&h.nats, &h.prefix, &h.server_id, "user:alice", "acme", id).await;
            if id == 4 {
                let body: Value = serde_json::from_slice(&resp.payload).expect("json");
                assert_rate_limited(&body, "caller");
            }
        }
        h.shutdown_tx.send(()).ok();
        h.join.await.expect("join").expect("gateway run");
    }

    #[tokio::test]
    #[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test rate_limit_caps -- --ignored"]
    async fn first_three_requests_within_budget_succeed() {
        let h = start_gateway(test_rate_limiter(), false).await;
        let subject = format!("{}.server.{}.tools.list", h.prefix.as_str(), h.server_id);
        spawn_fast_backend(h.nats.clone(), subject).await;
        for id in 1..=3 {
            let body: Value = serde_json::from_slice(
                &tools_list(&h.nats, &h.prefix, &h.server_id, "user:alice", "acme", id).await.payload,
            )
            .unwrap();
            assert!(body.get("result").is_some());
        }
        h.shutdown_tx.send(()).ok();
        h.join.await.expect("join").expect("gateway run");
    }
}

mod caller_isolation {
    use super::harness::{assert_rate_limited, spawn_fast_backend, start_gateway, test_rate_limiter, tools_list};
    use super::Value;

    #[tokio::test]
    #[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test rate_limit_caps -- --ignored"]
    async fn distinct_callers_do_not_share_rate_budget() {
        let h = start_gateway(test_rate_limiter(), false).await;
        let subject = format!("{}.server.{}.tools.list", h.prefix.as_str(), h.server_id);
        spawn_fast_backend(h.nats.clone(), subject).await;
        for id in 1..=4 {
            tools_list(&h.nats, &h.prefix, &h.server_id, "user:alice", "acme", id).await;
        }
        let bob = tools_list(&h.nats, &h.prefix, &h.server_id, "user:bob", "acme", 10).await;
        let body: Value = serde_json::from_slice(&bob.payload).expect("json");
        assert!(body.get("result").is_some(), "bob should succeed: {body}");
        let alice = tools_list(&h.nats, &h.prefix, &h.server_id, "user:alice", "acme", 11).await;
        let alice_body: Value = serde_json::from_slice(&alice.payload).expect("json");
        assert_rate_limited(&alice_body, "caller");
        h.shutdown_tx.send(()).ok();
        h.join.await.expect("join").expect("gateway run");
    }

    #[tokio::test]
    #[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test rate_limit_caps -- --ignored"]
    async fn same_caller_sub_reuses_budget_across_requests() {
        let h = start_gateway(test_rate_limiter(), false).await;
        let subject = format!("{}.server.{}.tools.list", h.prefix.as_str(), h.server_id);
        spawn_fast_backend(h.nats.clone(), subject).await;
        for id in 1..=4 {
            let resp = tools_list(&h.nats, &h.prefix, &h.server_id, "user:alice", "acme", id).await;
            if id == 4 {
                assert_rate_limited(&serde_json::from_slice(&resp.payload).unwrap(), "caller");
            }
        }
        h.shutdown_tx.send(()).ok();
        h.join.await.expect("join").expect("gateway run");
    }

    #[tokio::test]
    #[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test rate_limit_caps -- --ignored"]
    async fn exhausted_caller_does_not_affect_other_tenant_callers() {
        let h = start_gateway(test_rate_limiter(), false).await;
        let subject = format!("{}.server.{}.tools.list", h.prefix.as_str(), h.server_id);
        spawn_fast_backend(h.nats.clone(), subject).await;
        for id in 1..=4 {
            tools_list(&h.nats, &h.prefix, &h.server_id, "user:alice", "tenant_a", id).await;
        }
        let bob = tools_list(&h.nats, &h.prefix, &h.server_id, "user:bob", "tenant_b", 20).await;
        let body: Value = serde_json::from_slice(&bob.payload).expect("json");
        assert!(body.get("result").is_some(), "tenant_b bob should succeed: {body}");
        h.shutdown_tx.send(()).ok();
        h.join.await.expect("join").expect("gateway run");
    }
}

mod retry_after_header {
    use super::harness::{assert_rate_limited, spawn_fast_backend, start_gateway, test_rate_limiter, tools_list};
    use super::Value;

    #[tokio::test]
    #[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test rate_limit_caps -- --ignored"]
    async fn nats_reply_sets_retry_after_ms_header() {
        let h = start_gateway(test_rate_limiter(), false).await;
        let subject = format!("{}.server.{}.tools.list", h.prefix.as_str(), h.server_id);
        spawn_fast_backend(h.nats.clone(), subject).await;
        for id in 1..=4 {
            let resp = tools_list(&h.nats, &h.prefix, &h.server_id, "user:alice", "acme", id).await;
            if id == 4 {
                let hdr = resp.headers.expect("headers");
                let retry = hdr
                    .get("retry-after-ms")
                    .expect("retry-after-ms header")
                    .as_str()
                    .parse::<u64>()
                    .expect("integer");
                assert!(retry > 0);
            }
        }
        h.shutdown_tx.send(()).ok();
        h.join.await.expect("join").expect("gateway run");
    }

    #[tokio::test]
    #[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test rate_limit_caps -- --ignored"]
    async fn header_retry_after_ms_matches_jsonrpc_data_field() {
        let h = start_gateway(test_rate_limiter(), false).await;
        let subject = format!("{}.server.{}.tools.list", h.prefix.as_str(), h.server_id);
        spawn_fast_backend(h.nats.clone(), subject).await;
        for id in 1..=3 {
            tools_list(&h.nats, &h.prefix, &h.server_id, "user:alice", "acme", id).await;
        }
        let resp = tools_list(&h.nats, &h.prefix, &h.server_id, "user:alice", "acme", 4).await;
        let body: Value = serde_json::from_slice(&resp.payload).expect("json");
        let retry_body = assert_rate_limited(&body, "caller");
        let hdr = resp.headers.expect("headers");
        let retry_hdr = hdr.get("retry-after-ms").unwrap().as_str().parse::<u64>().unwrap();
        assert_eq!(retry_hdr, retry_body);
        h.shutdown_tx.send(()).ok();
        h.join.await.expect("join").expect("gateway run");
    }
}

mod window_recovery {
    use super::harness::{
        TEST_CALLER_WINDOW, assert_rate_limited, spawn_fast_backend, start_gateway, test_rate_limiter,
        tools_list,
    };
    use super::{Duration, Value};

    #[tokio::test]
    #[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test rate_limit_caps -- --ignored"]
    async fn caller_can_request_after_window_elapses() {
        let h = start_gateway(test_rate_limiter(), false).await;
        let subject = format!("{}.server.{}.tools.list", h.prefix.as_str(), h.server_id);
        spawn_fast_backend(h.nats.clone(), subject).await;
        for id in 1..=4 {
            tools_list(&h.nats, &h.prefix, &h.server_id, "user:alice", "acme", id).await;
        }
        tokio::time::sleep(TEST_CALLER_WINDOW + Duration::from_millis(100)).await;
        let resp = tools_list(&h.nats, &h.prefix, &h.server_id, "user:alice", "acme", 5).await;
        let body: Value = serde_json::from_slice(&resp.payload).expect("json");
        assert!(body.get("result").is_some(), "budget should reset: {body}");
        h.shutdown_tx.send(()).ok();
        h.join.await.expect("join").expect("gateway run");
    }

    #[tokio::test]
    #[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test rate_limit_caps -- --ignored"]
    async fn retry_after_ms_bounds_recovery_sleep() {
        let h = start_gateway(test_rate_limiter(), false).await;
        let subject = format!("{}.server.{}.tools.list", h.prefix.as_str(), h.server_id);
        spawn_fast_backend(h.nats.clone(), subject).await;
        for id in 1..=3 {
            tools_list(&h.nats, &h.prefix, &h.server_id, "user:alice", "acme", id).await;
        }
        let deny = tools_list(&h.nats, &h.prefix, &h.server_id, "user:alice", "acme", 4).await;
        let body: Value = serde_json::from_slice(&deny.payload).expect("json");
        let retry_ms = assert_rate_limited(&body, "caller");
        tokio::time::sleep(Duration::from_millis(retry_ms + 50)).await;
        let ok = tools_list(&h.nats, &h.prefix, &h.server_id, "user:alice", "acme", 5).await;
        let ok_body: Value = serde_json::from_slice(&ok.payload).expect("json");
        assert!(ok_body.get("result").is_some());
        h.shutdown_tx.send(()).ok();
        h.join.await.expect("join").expect("gateway run");
    }

    #[tokio::test]
    #[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test rate_limit_caps -- --ignored"]
    async fn immediate_retry_after_deny_still_rate_limited() {
        let h = start_gateway(test_rate_limiter(), false).await;
        let subject = format!("{}.server.{}.tools.list", h.prefix.as_str(), h.server_id);
        spawn_fast_backend(h.nats.clone(), subject).await;
        for id in 1..=4 {
            tools_list(&h.nats, &h.prefix, &h.server_id, "user:alice", "acme", id).await;
        }
        let retry = tools_list(&h.nats, &h.prefix, &h.server_id, "user:alice", "acme", 5).await;
        let body: Value = serde_json::from_slice(&retry.payload).expect("json");
        assert_rate_limited(&body, "caller");
        h.shutdown_tx.send(()).ok();
        h.join.await.expect("join").expect("gateway run");
    }
}

mod audit_rate_limited {
    use super::harness::{spawn_slow_backend, start_gateway, test_rate_limiter, tools_list, BACKEND_HOLD};
    use super::Duration;
    use futures::StreamExt;
    use trogon_mcp_gateway::audit::audit_publish_subject;

    #[tokio::test]
    #[ignore = "needs NATS JetStream (set NATS_URL). Run: cargo test -p trogon-mcp-gateway --test rate_limit_caps -- --ignored"]
    async fn rate_limited_publishes_audit_with_outcome_rate_limited() {
        let h = start_gateway(test_rate_limiter(), true).await;
        let audit_subject = audit_publish_subject(h.prefix.as_str(), "rate_limited", "request", "tools");
        let mut audit_sub = h.nats.subscribe(audit_subject.clone()).await.expect("audit sub");

        let subject = format!("{}.server.{}.tools.list", h.prefix.as_str(), h.server_id);
        spawn_slow_backend(h.nats.clone(), subject, BACKEND_HOLD).await;

        let nats = h.nats.clone();
        let prefix = h.prefix.clone();
        let server_id = h.server_id.clone();
        tokio::spawn(async move { tools_list(&nats, &prefix, &server_id, "a", "acme", 1).await });
        let nats = h.nats.clone();
        let prefix = h.prefix.clone();
        let server_id = h.server_id.clone();
        tokio::spawn(async move { tools_list(&nats, &prefix, &server_id, "b", "acme", 2).await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        tools_list(&h.nats, &h.prefix, &h.server_id, "c", "acme", 3).await;

        let audit_msg = tokio::time::timeout(Duration::from_secs(3), audit_sub.next())
            .await
            .expect("audit timeout")
            .expect("audit message");
        assert_eq!(audit_msg.subject.as_str(), audit_subject.as_str());
        let envelope: serde_json::Value = serde_json::from_slice(&audit_msg.payload).expect("audit json");
        assert_eq!(envelope.get("outcome").and_then(|v| v.as_str()), Some("rate_limited"));
        h.shutdown_tx.send(()).ok();
        h.join.await.expect("join").expect("gateway run");
    }

    #[tokio::test]
    #[ignore = "needs NATS JetStream (set NATS_URL). Run: cargo test -p trogon-mcp-gateway --test rate_limit_caps -- --ignored"]
    async fn audit_envelope_includes_rate_limit_scope() {
        let h = start_gateway(test_rate_limiter(), true).await;
        let audit_subject = audit_publish_subject(h.prefix.as_str(), "rate_limited", "request", "tools");
        let mut audit_sub = h.nats.subscribe(audit_subject).await.expect("audit sub");

        for server in ["x", "y", "z"] {
            let subject = format!("{}.server.{server}.tools.list", h.prefix.as_str());
            spawn_slow_backend(h.nats.clone(), subject, BACKEND_HOLD).await;
        }
        let nats = h.nats.clone();
        let prefix = h.prefix.clone();
        tokio::spawn(async move { tools_list(&nats, &prefix, "x", "a", "acme", 1).await });
        let nats = h.nats.clone();
        let prefix = h.prefix.clone();
        tokio::spawn(async move { tools_list(&nats, &prefix, "y", "b", "acme", 2).await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        tools_list(&h.nats, &h.prefix, "z", "c", "acme", 3).await;

        let audit_msg = tokio::time::timeout(Duration::from_secs(3), audit_sub.next())
            .await
            .expect("timeout")
            .expect("audit");
        let envelope: serde_json::Value = serde_json::from_slice(&audit_msg.payload).unwrap();
        assert_eq!(envelope.get("rate_limit_scope").and_then(|v| v.as_str()), Some("tenant"));
        h.shutdown_tx.send(()).ok();
        h.join.await.expect("join").expect("gateway run");
    }

    #[tokio::test]
    #[ignore = "needs NATS JetStream (set NATS_URL). Run: cargo test -p trogon-mcp-gateway --test rate_limit_caps -- --ignored"]
    async fn audit_envelope_includes_retry_after_ms() {
        let h = start_gateway(test_rate_limiter(), true).await;
        let audit_subject = audit_publish_subject(h.prefix.as_str(), "rate_limited", "request", "tools");
        let mut audit_sub = h.nats.subscribe(audit_subject).await.expect("audit sub");
        let subject = format!("{}.server.{}.tools.list", h.prefix.as_str(), h.server_id);
        super::harness::spawn_fast_backend(h.nats.clone(), subject).await;
        for id in 1..=4 {
            tools_list(&h.nats, &h.prefix, &h.server_id, "user:alice", "acme", id).await;
        }
        let audit_msg = tokio::time::timeout(Duration::from_secs(3), audit_sub.next())
            .await
            .expect("timeout")
            .expect("audit");
        let envelope: serde_json::Value = serde_json::from_slice(&audit_msg.payload).unwrap();
        assert!(envelope.get("retry_after_ms").and_then(|v| v.as_u64()).unwrap_or(0) > 0);
        h.shutdown_tx.send(()).ok();
        h.join.await.expect("join").expect("gateway run");
    }
}
