//! Integration tests for gateway mesh egress minting with a stub STS responder.

use std::sync::Arc;
use std::time::Duration;

use async_nats::HeaderMap;
use futures::StreamExt;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use mcp_nats::{Config as McpConfig, McpPrefix};
use rsa::RsaPrivateKey;
use rsa::pkcs8::EncodePrivateKey;
use serde_json::json;
use trogon_mcp_gateway::agent_identity::AgentIdentityMode;
use trogon_mcp_gateway::authz::AllowAllPermissionChecker;
use trogon_mcp_gateway::egress::{EgressMintConfig, EgressMinter, backend_target_aud};
use trogon_mcp_gateway::gateway::GatewaySettings;
use trogon_mcp_gateway::jwt::{JwtIngressConfig, JwtMode, JwtValidator};
use trogon_nats::{NatsAuth, NatsConfig, connect};
use trogon_sts_client::{StsClient, StsClientConfig};
use uuid::Uuid;

const HS_SECRET: &[u8] = b"mesh-egress-test-secret-at-least-32-bytes!!";
const HS_ISSUER: &str = "https://bootstrap.test/";
const GATEWAY_AUD: &str = "urn:trogon:mcp:gateway:acme:gw-test";

fn mesh_signer() -> (EncodingKey, String) {
    let mut rng = rand::thread_rng();
    let private = RsaPrivateKey::new(&mut rng, 2048).expect("rsa");
    let pem = private
        .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
        .expect("pem")
        .to_string();
    (EncodingKey::from_rsa_pem(pem.as_bytes()).expect("enc"), "urn:trogon:sts:mesh".into())
}

fn mint_mesh_jwt(enc: &EncodingKey, mesh_iss: &str, aud: &str, sub: &str, gateway_wkl: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let claims = json!({
        "iss": mesh_iss,
        "sub": sub,
        "aud": aud,
        "exp": now + 120,
        "iat": now,
        "tenant": "acme",
        "wkl": gateway_wkl,
        "act_chain": [
            {"sub": sub, "wkl": "sentinel:human", "iat": now},
            {"sub": sub, "agent_id": null, "wkl": gateway_wkl, "iat": now}
        ]
    });
    encode(
        &Header::new(Algorithm::RS256),
        &claims,
        enc,
    )
    .expect("sign mesh jwt")
}

fn bootstrap_jwt(sub: &str) -> String {
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
        "tenant": "acme",
        "https://trogon.ai/tenant": "acme",
        "session_id": "sess-integration",
        "purpose": "test.purpose"
    });
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(HS_SECRET),
    )
    .expect("sign bootstrap jwt")
}

fn jwt_validator() -> Arc<JwtValidator> {
    JwtValidator::try_new(JwtIngressConfig {
            mode: JwtMode::Require,
            agent_identity_mode: AgentIdentityMode::Enforce,
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

#[tokio::test]
#[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test egress_mint -- --ignored"]
async fn backend_receives_mesh_token_not_inbound_bootstrap() {
    let url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let connect_timeout = Duration::from_secs(15);
    let nats_conf = NatsConfig::new(vec![url], NatsAuth::None);

    let prefix_segment = format!("{}", Uuid::now_v7().as_simple());
    let prefix_token = format!("{}.mcp", prefix_segment);
    let prefix = McpPrefix::new(&prefix_token).expect("test prefix shape");
    let mcp_conf = McpConfig::new(prefix.clone(), nats_conf.clone()).with_operation_timeout(Duration::from_secs(15));

    let (mesh_enc, mesh_iss) = mesh_signer();
    let gateway_wkl = "spiffe://acme.local/ns/prod/sa/mcp-gateway";
    let server_id = "github";
    let backend_aud = backend_target_aud("acme", server_id);
    let inbound = bootstrap_jwt("oidc|acme|alice");

    let nats = Arc::new(connect(&nats_conf, connect_timeout).await.expect("nats connect"));

    let sts_subject = format!("sts-{prefix_segment}");
    {
        let mesh_enc = mesh_enc.clone();
        let mesh_iss = mesh_iss.clone();
        let backend_aud = backend_aud.clone();
        let inbound_for_assert = inbound.clone();
        let nats_sts = nats.clone();
        let mut sub = nats_sts.subscribe(sts_subject.clone()).await.expect("sts sub");
        tokio::spawn(async move {
            while let Some(msg) = sub.next().await {
                let req: serde_json::Value =
                    serde_json::from_slice(&msg.payload).expect("sts request json");
                assert_eq!(req["subject_token"].as_str(), Some(inbound_for_assert.as_str()));
                assert_eq!(req["audience"].as_str(), Some(backend_aud.as_str()));
                assert_eq!(req["actor_token"].as_str(), Some(gateway_wkl));

                let mesh = mint_mesh_jwt(&mesh_enc, &mesh_iss, &backend_aud, "oidc|acme|alice", gateway_wkl);
                let body = json!({
                    "access_token": mesh,
                    "issued_token_type": "urn:ietf:params:oauth:token-type:jwt",
                    "token_type": "Bearer",
                    "expires_in": 120
                });
                if let Some(reply) = msg.reply {
                    nats_sts
                        .publish(reply.to_string(), serde_json::to_vec(&body).unwrap().into())
                        .await
                        .ok();
                    nats_sts.flush().await.ok();
                }
            }
        });
    }

    let subject_out = format!("{}.server.{server_id}.tools.list", prefix.as_str());
    let captured = Arc::new(tokio::sync::Mutex::new(None::<(String, Option<String>)>));
    {
        let captured = captured.clone();
        let nats_backend = nats.clone();
        let mut sub = nats_backend.subscribe(subject_out.clone()).await.expect("backend sub");
        tokio::spawn(async move {
            if let Some(msg) = sub.next().await {
                let auth = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get("authorization").or_else(|| h.get("Authorization")))
                    .map(|v| v.as_str().to_string());
                let caller_jwt = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get("A2a-Caller-Jwt"))
                    .map(|v| v.as_str().to_string());
                *captured.lock().await = Some((auth.unwrap_or_default(), caller_jwt));
                if let Some(reply) = msg.reply {
                    let body = json!({"jsonrpc":"2.0","id":1,"result":{"tools":[]}});
                    nats_backend
                        .publish(reply.to_string(), serde_json::to_vec(&body).unwrap().into())
                        .await
                        .ok();
                    nats_backend.flush().await.ok();
                }
            }
        });
    }

    let sts = StsClient::new(
        (*nats).clone(),
        StsClientConfig {
            exchange_subject: sts_subject,
            timeout: Duration::from_secs(5),
        },
    );
    let egress = EgressMinter::from_parts(
        sts,
        EgressMintConfig {
            mesh_token_ttl_secs: 120,
            cache_max_entries: 100,
            actor_token: gateway_wkl.into(),
            clock_skew_secs: 30,
        },
        None,
    );

    let settings = GatewaySettings {
        queue_group: format!("q-{prefix_segment}"),
        audit_stream_name: "MCP_AUDIT_E2E".into(),
        init_audit_stream: false,
        mcp: mcp_conf,
        jwt: jwt_validator(),
        egress: Some(egress),
        chain_resolver: None,
        rate_limit: None,
        approval_gate: None,
        mesh_config: trogon_mcp_gateway::policy::MeshGatewayConfig::default(),
        context_throttle: None,
        anomaly_emitter: None,
    };

    let checker: Arc<dyn trogon_mcp_gateway::authz::PermissionChecker> = Arc::new(AllowAllPermissionChecker);
    let traces = trogon_mcp_gateway::trace::TraceStore::default();
    let gateway_client = nats.clone();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let join = tokio::spawn(async move {
        trogon_mcp_gateway::run(gateway_client, checker, traces, settings, async {
            shutdown_rx.await.ok();
        })
        .await
    });

    tokio::time::sleep(Duration::from_millis(400)).await;

    let mut headers = HeaderMap::new();
    headers.insert("authorization", format!("Bearer {inbound}").as_str());
    let ingress = format!("{}.gateway.request.{server_id}.tools.list", prefix.as_str());
    let payload = json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}});
    let response = nats
        .request_with_headers(
            ingress,
            headers,
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("gateway request");

    let body: serde_json::Value = serde_json::from_slice(&response.payload).expect("json-rpc");
    assert!(body.get("result").is_some(), "expected success, got {body}");

    let captured = captured.lock().await.take().expect("backend should have received request");
    let auth = captured.0;
    assert!(
        auth.contains("Bearer"),
        "backend should receive Authorization bearer mesh token"
    );
    assert!(
        !auth.contains(inbound.as_str()),
        "backend must not receive inbound bootstrap token"
    );
    assert!(captured.1.is_none(), "A2a-Caller-Jwt must be stripped");

    let mesh_token = auth.trim_start_matches("Bearer ").trim();
    let parts: Vec<&str> = mesh_token.split('.').collect();
    assert_eq!(parts.len(), 3);
    let payload_bytes = base64_decode_url(parts[1]);
    let claims: serde_json::Value = serde_json::from_slice(&payload_bytes).unwrap();
    assert_eq!(claims["aud"].as_str(), Some(backend_aud.as_str()));
    assert_eq!(claims["iss"].as_str(), Some(mesh_iss.as_str()));
    let chain = claims["act_chain"].as_array().expect("act_chain array");
    assert!(
        chain.iter().any(|e| e.get("wkl").and_then(|v| v.as_str()) == Some(gateway_wkl)),
        "act_chain should include gateway workload hop"
    );

    shutdown_tx.send(()).ok();
    join.await.expect("join").expect("gateway run");
}

#[tokio::test]
#[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway --test egress_mint -- --ignored"]
async fn sts_timeout_returns_structured_error() {
    let url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let connect_timeout = Duration::from_secs(15);
    let nats_conf = NatsConfig::new(vec![url], NatsAuth::None);
    let prefix_segment = format!("{}", Uuid::now_v7().as_simple());
    let prefix = McpPrefix::new(format!("{prefix_segment}.mcp")).expect("prefix");
    let mcp_conf = McpConfig::new(prefix.clone(), nats_conf.clone()).with_operation_timeout(Duration::from_secs(5));
    let nats = Arc::new(connect(&nats_conf, connect_timeout).await.expect("nats"));

    let sts = StsClient::new(
        (*nats).clone(),
        StsClientConfig {
            exchange_subject: format!("sts-dead-{prefix_segment}"),
            timeout: Duration::from_millis(50),
        },
    );
    let egress = EgressMinter::from_parts(
        sts,
        EgressMintConfig {
            mesh_token_ttl_secs: 120,
            cache_max_entries: 100,
            actor_token: "spiffe://acme.local/ns/prod/sa/mcp-gateway".into(),
            clock_skew_secs: 30,
        },
        None,
    );

    let settings = GatewaySettings {
        queue_group: format!("q-{prefix_segment}"),
        audit_stream_name: "MCP_AUDIT".into(),
        init_audit_stream: false,
        mcp: mcp_conf,
        jwt: jwt_validator(),
        egress: Some(egress),
        chain_resolver: None,
        rate_limit: None,
        approval_gate: None,
        mesh_config: trogon_mcp_gateway::policy::MeshGatewayConfig::default(),
        context_throttle: None,
        anomaly_emitter: None,
    };

    let checker: Arc<dyn trogon_mcp_gateway::authz::PermissionChecker> = Arc::new(AllowAllPermissionChecker);
    let traces = trogon_mcp_gateway::trace::TraceStore::default();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let join = tokio::spawn({
        let nats = nats.clone();
        async move {
            trogon_mcp_gateway::run(nats, checker, traces, settings, async {
                shutdown_rx.await.ok();
            })
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let inbound = bootstrap_jwt("oidc|acme|bob");
    let mut headers = HeaderMap::new();
    headers.insert("authorization", format!("Bearer {inbound}").as_str());
    let ingress = format!("{}.gateway.request.github.tools.list", prefix.as_str());
    let payload = json!({"jsonrpc":"2.0","id":7,"method":"tools/list","params":{}});
    let response = nats
        .request_with_headers(
            ingress,
            headers,
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("gateway reply");

    let body: serde_json::Value = serde_json::from_slice(&response.payload).unwrap();
    assert_eq!(body["error"]["code"], -32_107);
    assert_eq!(body["error"]["message"], "sts_unavailable");

    shutdown_tx.send(()).ok();
    join.await.expect("join").expect("run");
}

fn base64_decode_url(input: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input)
        .expect("b64")
}
