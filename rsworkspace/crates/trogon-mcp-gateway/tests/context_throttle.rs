//! Integration tests for per-context `(tenant_id, agent_id, purpose)` token-bucket throttling.

use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use mcp_nats::{Config as McpConfig, McpPrefix};
use serde_json::{Value, json};
use trogon_mcp_gateway::authz::AllowAllPermissionChecker;
use trogon_mcp_gateway::context_throttle::{
    ContextBudget, ContextThrottle, ContextThrottleConfig, TestClock,
};
use trogon_mcp_gateway::gateway::GatewaySettings;
use trogon_mcp_gateway::rpc_codes;
use trogon_nats::{NatsAuth, NatsConfig, connect};
use uuid::Uuid;

const RATE_LIMITED: i32 = rpc_codes::RATE_LIMITED;

mod harness {
    use super::*;

    const HS_SECRET: &[u8] = b"context-throttle-test-secret-at-least-32b!";
    const HS_ISSUER: &str = "https://context-throttle.test/";
    const GATEWAY_AUD: &str = "urn:trogon:mcp:gateway:acme:ctx-throttle";

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

    pub fn jwt_for_context(
        sub: &str,
        tenant: &str,
        agent_id: Option<&str>,
        purpose: Option<&str>,
    ) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let mut claims = json!({
            "iss": HS_ISSUER,
            "sub": sub,
            "aud": GATEWAY_AUD,
            "exp": now + 3600,
            "iat": now,
            "https://trogon.ai/tenant": tenant,
        });
        if let Some(agent_id) = agent_id {
            claims["agent_id"] = json!(agent_id);
        }
        if let Some(purpose) = purpose {
            claims["purpose"] = json!(purpose);
        }
        encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(HS_SECRET),
        )
        .expect("sign jwt")
    }

    pub struct GatewayHarness {
        pub nats: Arc<async_nats::Client>,
        pub prefix: McpPrefix,
        pub server_id: String,
        pub shutdown_tx: tokio::sync::oneshot::Sender<()>,
        pub join: tokio::task::JoinHandle<Result<(), trogon_mcp_gateway::GatewayError>>,
    }

    pub async fn start_gateway(context_throttle: Arc<ContextThrottle>) -> GatewayHarness {
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
            audit_stream_name: format!("MCP_AUDIT_CTX_{prefix_segment}"),
            init_audit_stream: false,
            mcp: mcp_conf,
            jwt: jwt_validator(),
            egress: None,
            chain_resolver: None,
            rate_limit: None,
            approval_gate: None,
            mesh_config: trogon_mcp_gateway::policy::MeshGatewayConfig::default(),
            context_throttle: Some(context_throttle),
            anomaly_emitter: None,
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

    pub async fn spawn_fast_backend(nats: Arc<async_nats::Client>, subject_out: String) {
        use futures::StreamExt;
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
        agent_id: Option<&str>,
        purpose: Option<&str>,
        request_id: i64,
    ) -> async_nats::Message {
        let ingress = format!("{}.gateway.request.{server_id}.tools.list", prefix.as_str());
        let token = jwt_for_context(sub, tenant, agent_id, purpose);
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

    pub fn assert_rate_limited(body: &Value, scope: &str) {
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
    }

    pub fn throttle_with_burst(burst: u32, clock: TestClock) -> Arc<ContextThrottle> {
        Arc::new(ContextThrottle::with_clock(
            ContextThrottleConfig {
                default_budget: ContextBudget {
                    tokens_per_minute: 60,
                    burst,
                },
                ..ContextThrottleConfig::default()
            },
            clock,
        ))
    }
}

#[tokio::test]
#[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test context_throttle -- --ignored"]
async fn context_throttle_allows_under_budget() {
    use harness::{spawn_fast_backend, start_gateway, throttle_with_burst, tools_list};

    let clock = TestClock::new(Instant::now());
    let h = start_gateway(throttle_with_burst(5, clock.clone())).await;
    let subject = format!("{}.server.{}.tools.list", h.prefix.as_str(), h.server_id);
    spawn_fast_backend(h.nats.clone(), subject).await;

    let resp = tools_list(
        &h.nats,
        &h.prefix,
        &h.server_id,
        "user:alice",
        "acme",
        Some("agent/oncall"),
        Some("incident"),
        1,
    )
    .await;
    let body: Value = serde_json::from_slice(&resp.payload).expect("json");
    assert!(body.get("result").is_some(), "expected success under budget, got {body}");

    clock.advance(Duration::from_secs(1));
    let resp2 = tools_list(
        &h.nats,
        &h.prefix,
        &h.server_id,
        "user:alice",
        "acme",
        Some("agent/oncall"),
        Some("incident"),
        2,
    )
    .await;
    let body2: Value = serde_json::from_slice(&resp2.payload).expect("json");
    assert!(body2.get("result").is_some(), "expected refill after clock advance, got {body2}");

    h.shutdown_tx.send(()).ok();
    h.join.await.expect("join").expect("gateway run");
}

#[tokio::test]
#[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test context_throttle -- --ignored"]
async fn context_throttle_throttles_with_scope_purpose() {
    use harness::{assert_rate_limited, spawn_fast_backend, start_gateway, throttle_with_burst, tools_list};

    let h = start_gateway(throttle_with_burst(2, TestClock::new(Instant::now()))).await;
    let subject = format!("{}.server.{}.tools.list", h.prefix.as_str(), h.server_id);
    spawn_fast_backend(h.nats.clone(), subject).await;

    for id in 1..=2 {
        let resp = tools_list(
            &h.nats,
            &h.prefix,
            &h.server_id,
            "user:alice",
            "acme",
            Some("agent/oncall"),
            Some("deploy"),
            id,
        )
        .await;
        let body: Value = serde_json::from_slice(&resp.payload).expect("json");
        assert!(body.get("result").is_some(), "request {id} should succeed");
    }

    let third = tools_list(
        &h.nats,
        &h.prefix,
        &h.server_id,
        "user:alice",
        "acme",
        Some("agent/oncall"),
        Some("deploy"),
        3,
    )
    .await;
    let body: Value = serde_json::from_slice(&third.payload).expect("json");
    assert_rate_limited(&body, "purpose");

    h.shutdown_tx.send(()).ok();
    h.join.await.expect("join").expect("gateway run");
}

#[tokio::test]
#[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test context_throttle -- --ignored"]
async fn context_throttle_skipped_when_key_incomplete() {
    use harness::{spawn_fast_backend, start_gateway, throttle_with_burst, tools_list};

    let h = start_gateway(throttle_with_burst(1, TestClock::new(Instant::now()))).await;
    let subject = format!("{}.server.{}.tools.list", h.prefix.as_str(), h.server_id);
    spawn_fast_backend(h.nats.clone(), subject).await;

    for id in 1..=3 {
        let resp = tools_list(
            &h.nats,
            &h.prefix,
            &h.server_id,
            "user:alice",
            "acme",
            None,
            Some("deploy"),
            id,
        )
        .await;
        let body: Value = serde_json::from_slice(&resp.payload).expect("json");
        assert!(
            body.get("result").is_some(),
            "request {id} without agent_id should bypass context throttle, got {body}"
        );
    }

    h.shutdown_tx.send(()).ok();
    h.join.await.expect("join").expect("gateway run");
}
