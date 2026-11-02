use std::convert::Infallible;
use std::time::Duration;

use a2a_auth_callout::admission_keys::CalloutXkeyError;
use a2a_auth_callout::{BridgeCredential, BridgeCredentialError, BridgeMintRequest, BridgeMintResponse, CalloutXkey};
use a2a_bridge::edge_auth::{ENV_AAUTH_AUTHORITY, ENV_AAUTH_JWKS_PATH, ENV_AAUTH_MODE, ENV_AAUTH_RESOURCE_ISS};
use a2a_bridge::{AuthCalloutClient, CallerHttpsAuth, StubAuthCalloutMint};
use async_nats::jetstream::kv;
use async_nats::jetstream::stream::StorageType;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use bytes::Bytes;
use futures_util::StreamExt;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use p256::ecdsa::SigningKey;
use p256::pkcs8::LineEnding;
use pkcs8::EncodePrivateKey as _;
use rand_core::{OsRng, RngCore as _};
use serde_json::json;
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use trogon_aauth_verify::{ReplayBucketSpec, SystemTimeSource, TimeSource};
use trogon_identity_types::aauth::TYP_AGENT;
use trogon_identity_types::aauth::headers::{CONTENT_DIGEST, SIGNATURE, SIGNATURE_INPUT, SIGNATURE_KEY};
use trogon_nats::NatsTls;
use trogon_nats::test_support::{CoreTestServer, JetStreamTestServer};
use trogon_std::env::InMemoryEnv;
use trogon_std::fs::MemFs;
use trogon_std::log_capture::{CapturedEvents, CapturedLogs, LevelFilter, LogLevel};

use super::*;

#[derive(Debug, thiserror::Error)]
enum TestError {
    #[error(transparent)]
    Bridge(#[from] BridgeError),
    #[error(transparent)]
    Bootstrap(#[from] BootstrapError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Connect(#[from] trogon_nats::ConnectError),
    #[error(transparent)]
    Subscribe(#[from] async_nats::SubscribeError),
    #[error(transparent)]
    Unsubscribe(#[from] async_nats::UnsubscribeError),
    #[error(transparent)]
    Flush(#[from] async_nats::client::FlushError),
    #[error(transparent)]
    Publish(#[from] async_nats::PublishError),
    #[error(transparent)]
    Timeout(#[from] tokio::time::error::Elapsed),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Jwt(#[from] jsonwebtoken::errors::Error),
    #[error(transparent)]
    Pkcs8(#[from] p256::pkcs8::Error),
    #[error(transparent)]
    Http(#[from] axum::http::Error),
    #[error(transparent)]
    Body(#[from] axum::Error),
    #[error(transparent)]
    Router(#[from] Infallible),
    #[error(transparent)]
    Nkeys(#[from] nkeys::error::Error),
    #[error(transparent)]
    CalloutXkey(#[from] CalloutXkeyError),
    #[error(transparent)]
    SealedCredential(#[from] BridgeCredentialError),
    #[error(transparent)]
    CreateKeyValue(#[from] async_nats::jetstream::context::CreateKeyValueError),
    #[error("request or reply subject missing")]
    MissingMessage,
    #[error("mint request carries no sealed credential")]
    MissingSealedCredential,
    #[error("generated signing key has no affine coordinates")]
    MissingSigningCoordinate,
}

fn callout_xkey() -> Result<CalloutXkey, TestError> {
    Ok(CalloutXkey::from_seed(&nkeys::XKey::new().seed()?)?)
}

fn rpc_request() -> Result<Request<Body>, TestError> {
    rpc_request_with(r#"{"jsonrpc":"2.0","id":"bootstrap-request","method":"message/send","params":{}}"#)
}

fn rpc_request_with(body: &str) -> Result<Request<Body>, TestError> {
    Ok(Request::post("/")
        .header("authorization", "Bearer caller-fixture")
        .header("x-a2a-agent-id", "planner")
        .body(Body::from(body.to_owned()))?)
}

struct SigningFixture {
    provider: EncodingKey,
    provider_jwk: serde_json::Value,
    key: EncodingKey,
    key_jwk: serde_json::Value,
}

impl SigningFixture {
    fn new() -> Result<Self, TestError> {
        let (provider, provider_jwk) = signing_key("provider-1")?;
        let (key, key_jwk) = signing_key("agent-key")?;
        Ok(Self {
            provider,
            provider_jwk,
            key,
            key_jwk,
        })
    }

    fn jwks_document(&self) -> String {
        json!({"https://agents.test": {"keys": [self.provider_jwk]}}).to_string()
    }

    fn agent_token(&self) -> Result<String, TestError> {
        let mut header = Header::new(Algorithm::ES256);
        header.typ = Some(TYP_AGENT.into());
        header.kid = Some("provider-1".into());
        let issued = SystemTimeSource.now();
        Ok(jsonwebtoken::encode(
            &header,
            &json!({
                "iss": "https://agents.test",
                "sub": "aauth:agent-1@agents.test",
                "jti": URL_SAFE_NO_PAD.encode(OsRng.next_u64().to_be_bytes()),
                "iat": issued - 10,
                "exp": issued + 3600,
                "dwk": "aauth-agent.json",
                "cnf": {"jwk": self.key_jwk},
            }),
            &self.provider,
        )?)
    }

    fn request(&self) -> Result<Request<Body>, TestError> {
        let body = br#"{"jsonrpc":"2.0","id":"bootstrap-request","method":"message/send","params":{}}"#;
        let agent_token = self.agent_token()?;
        let signature_key = format!("sig=jwt;jwt=\"{agent_token}\"");
        let digest = format!("sha-256=:{}:", STANDARD.encode(Sha256::digest(body)));
        let created = SystemTimeSource.now();
        let jti = URL_SAFE_NO_PAD.encode(OsRng.next_u64().to_be_bytes());
        let params = format!(
            "(\"@method\" \"@authority\" \"@path\" \"signature-key\" \"content-digest\");created={created};expires={};nonce=\"{jti}\"",
            created + 60,
        );
        let base = format!(
            "\"@method\": POST\n\"@authority\": bridge.test\n\"@path\": /\n\"signature-key\": {signature_key}\n\"content-digest\": {digest}\n\"@signature-params\": {params}"
        );
        let signature = jsonwebtoken::crypto::sign(base.as_bytes(), &self.key, Algorithm::ES256)?;
        Ok(Request::post("/")
            .header("authorization", format!("Bearer {agent_token}"))
            .header("x-a2a-agent-id", "planner")
            .header(SIGNATURE_KEY, signature_key)
            .header(SIGNATURE_INPUT, format!("sig={params}"))
            .header(SIGNATURE, format!("sig=:{signature}:"))
            .header(CONTENT_DIGEST, digest)
            .body(Body::from(body.as_slice()))?)
    }
}

fn signing_key(kid: &str) -> Result<(EncodingKey, serde_json::Value), TestError> {
    let signing = SigningKey::random(&mut OsRng);
    let point = signing.verifying_key().to_encoded_point(false);
    let (Some(x), Some(y)) = (point.x(), point.y()) else {
        return Err(TestError::MissingSigningCoordinate);
    };
    let x = URL_SAFE_NO_PAD.encode(x);
    let y = URL_SAFE_NO_PAD.encode(y);
    let pem = signing.to_pkcs8_pem(LineEnding::LF)?.to_string();
    let encoding = EncodingKey::from_ec_pem(pem.as_bytes())?;
    Ok((
        encoding,
        json!({"kty": "EC", "crv": "P-256", "x": x, "y": y, "kid": kid, "alg": "ES256"}),
    ))
}

struct StallingProxy {
    address: String,
    accepting: tokio::task::JoinHandle<()>,
}

impl StallingProxy {
    async fn start(upstream: &str, relayed_connections: usize) -> Result<Self, TestError> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?.to_string();
        let upstream = upstream.to_owned();
        let accepting = tokio::spawn(async move {
            let mut stalled = Vec::new();
            let mut relayed = 0;
            while let Ok((mut inbound, _)) = listener.accept().await {
                if relayed < relayed_connections {
                    relayed += 1;
                    let upstream = upstream.clone();
                    tokio::spawn(async move {
                        if let Ok(mut outbound) = tokio::net::TcpStream::connect(upstream).await {
                            let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                        }
                    });
                } else {
                    stalled.push(inbound);
                }
            }
        });
        Ok(Self { address, accepting })
    }
}

impl Drop for StallingProxy {
    fn drop(&mut self) {
        self.accepting.abort();
    }
}

async fn answer_next_request(
    client: &async_nats::Client,
    subscription: &mut async_nats::Subscriber,
    reply: Bytes,
) -> Result<(), TestError> {
    let request = tokio::time::timeout(Duration::from_secs(5), subscription.next())
        .await?
        .ok_or(TestError::MissingMessage)?;
    client
        .publish(request.reply.ok_or(TestError::MissingMessage)?, reply)
        .await?;
    Ok(())
}

#[test]
fn timeout_input_accepts_trimmed_unsigned_values_and_rejects_invalid_values() {
    let env = InMemoryEnv::new();
    assert_eq!(parse_u64_env(&env, "TIMEOUT"), None);
    for (value, expected) in [(" 24 ", Some(24)), ("0", Some(0)), ("-1", None), ("overflow", None)] {
        env.set("TIMEOUT", value);
        assert_eq!(parse_u64_env(&env, "TIMEOUT"), expected);
    }
}

#[test]
fn bootstrap_prefix_uses_the_default_or_validated_supplied_namespace() -> Result<(), TestError> {
    let env = InMemoryEnv::new();
    assert_eq!(resolve_a2a_prefix(&env)?.as_str(), "a2a");
    env.set(ENV_A2A_PREFIX, "tenant");
    assert_eq!(resolve_a2a_prefix(&env)?.as_str(), "tenant");
    env.set(ENV_A2A_PREFIX, "invalid.*");
    assert!(matches!(resolve_a2a_prefix(&env), Err(BridgeError::NatsPublish(_))));
    Ok(())
}

#[tokio::test]
async fn run_rejects_invalid_configuration_before_serving() {
    let env = InMemoryEnv::new();
    env.set("BRIDGE_LISTEN_ADDR", "not-an-address");
    assert!(matches!(
        run(&env, &MemFs::new(), std::future::pending()).await,
        Err(BootstrapError::BadListenAddr)
    ));
    env.set("BRIDGE_LISTEN_ADDR", "127.0.0.1:0");
    unenforced(&env);
    env.set("A2A_BRIDGE_TRANSPORT", "unsupported");
    assert!(
        matches!(run(&env, &MemFs::new(), std::future::pending()).await, Err(BootstrapError::UnknownTransport(value)) if value == "unsupported")
    );
    env.set(ENV_A2A_PREFIX, "invalid.*");
    assert!(matches!(
        run(&env, &MemFs::new(), std::future::pending()).await,
        Err(BootstrapError::Bridge(_))
    ));
    env.set(ENV_A2A_PREFIX, "tenant");
    env.set("A2A_BRIDGE_TRANSPORT", "nats");
    env.set("NATS_URL", " , ");
    assert!(matches!(
        run(&env, &MemFs::new(), std::future::pending()).await,
        Err(BootstrapError::NoNatsServers)
    ));
}

#[tokio::test]
async fn run_reports_an_occupied_listener() -> Result<(), TestError> {
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let env = InMemoryEnv::new();
    env.set("BRIDGE_LISTEN_ADDR", occupied.local_addr()?.to_string());
    unenforced(&env);
    assert!(
        matches!(run(&env, &MemFs::new(), std::future::pending()).await, Err(BootstrapError::Listen(error)) if error.kind() == std::io::ErrorKind::AddrInUse)
    );
    Ok(())
}

#[tokio::test]
async fn run_completes_after_controlled_shutdown() {
    let env = InMemoryEnv::new();
    env.set("BRIDGE_LISTEN_ADDR", "127.0.0.1:0");
    unenforced(&env);
    assert!(run(&env, &MemFs::new(), std::future::ready(())).await.is_ok());
}

#[tokio::test]
async fn run_enforces_proof_of_possession_unless_told_otherwise() {
    let env = InMemoryEnv::new();
    env.set("BRIDGE_LISTEN_ADDR", "127.0.0.1:0");
    assert!(matches!(
        run(&env, &MemFs::new(), std::future::ready(())).await,
        Err(BootstrapError::EdgeAuth(EdgeAuthConfigError::MissingRequired(
            ENV_AAUTH_RESOURCE_ISS
        )))
    ));
}

#[tokio::test]
async fn run_refuses_a_mode_below_enforce_without_the_dev_flag() {
    for mode in [EdgeAuthMode::Shadow, EdgeAuthMode::Off] {
        let env = InMemoryEnv::new();
        env.set("BRIDGE_LISTEN_ADDR", "127.0.0.1:0");
        env.set(ENV_AAUTH_MODE, mode.as_str());
        assert!(
            matches!(
                run(&env, &MemFs::new(), std::future::ready(())).await,
                Err(BootstrapError::EdgeAuth(EdgeAuthConfigError::UnenforcedWithoutDevFlag(refused))) if refused == mode
            ),
            "{mode}"
        );
    }
}

#[tokio::test]
async fn run_warns_when_proof_of_possession_is_not_enforced() {
    {
        let quiet = CapturedEvents::new();
        let _guard = quiet.install(LevelFilter::ERROR);
        let env = InMemoryEnv::new();
        env.set("BRIDGE_LISTEN_ADDR", "127.0.0.1:0");
        unenforced(&env);
        assert!(run(&env, &MemFs::new(), std::future::ready(())).await.is_ok());
        assert!(quiet.events().is_empty());
    }
    let events = CapturedEvents::new();
    let _guard = events.install(LevelFilter::WARN);
    let env = InMemoryEnv::new();
    env.set("BRIDGE_LISTEN_ADDR", "127.0.0.1:0");
    unenforced(&env);
    assert!(run(&env, &MemFs::new(), std::future::ready(())).await.is_ok());
    let warning = events.events().into_iter().find(|event| {
        event.message()
            == Some("AAuth proof of possession is not enforced at the HTTP edge, which is only for development")
    });
    assert!(warning.is_some(), "the unenforced edge is diagnosed");
    if let Some(warning) = warning {
        assert_eq!(warning.field("mode"), Some("off"));
        assert_eq!(warning.field("flag"), Some(ENV_AAUTH_DEV_UNENFORCED));
    }
}

#[tokio::test]
async fn run_allows_explicit_shadow_mode_with_local_stub_replay() {
    {
        let quiet = CapturedEvents::new();
        let _guard = quiet.install(LevelFilter::ERROR);
        let env = InMemoryEnv::new();
        let fs = MemFs::new();
        env.set("BRIDGE_LISTEN_ADDR", "127.0.0.1:0");
        proof_settings(&env, &fs, "shadow");
        env.set(ENV_AAUTH_DEV_UNENFORCED, "true");
        assert!(run(&env, &fs, std::future::ready(())).await.is_ok());
        assert!(quiet.events().is_empty());
    }
    let events = CapturedEvents::new();
    let _guard = events.install(LevelFilter::WARN);
    let env = InMemoryEnv::new();
    let fs = MemFs::new();
    env.set("BRIDGE_LISTEN_ADDR", "127.0.0.1:0");
    proof_settings(&env, &fs, "shadow");
    env.set(ENV_AAUTH_DEV_UNENFORCED, "true");

    assert!(run(&env, &fs, std::future::ready(())).await.is_ok());
    let warning = events
        .events()
        .into_iter()
        .find(|event| event.message() == Some("AAuth replay records stay in this process on the stub transport"));
    assert!(warning.is_some(), "the stub replay scope is diagnosed");
    if let Some(warning) = warning {
        assert_eq!(warning.field("mode"), Some("shadow"));
    }
}

#[tokio::test]
async fn stub_bootstrap_refuses_enforce_without_a_shared_replay_store() -> Result<(), TestError> {
    let env = InMemoryEnv::new();
    let fs = MemFs::new();
    proof_settings(&env, &fs, "enforce");
    let result = bootstrap_stub_transport(
        &env,
        &NatsConfig::from_url("nats://unused:4222"),
        resolve_a2a_prefix(&env)?,
        EdgeAuthConfig::from_env(&env, &fs).map_err(BootstrapError::EdgeAuth)?,
    );
    assert!(matches!(result, Err(BootstrapError::EnforcedWithoutReplayStore)));
    Ok(())
}

#[tokio::test]
async fn nats_bootstrap_refuses_enforce_until_the_replay_bucket_is_provisioned() -> Result<(), TestError> {
    let server = JetStreamTestServer::start().await;
    let client = trogon_nats::connect(
        &NatsConfig::from_url(server.address()).with_tls(NatsTls::Disabled),
        Duration::from_secs(5),
    )
    .await?;
    let env = InMemoryEnv::new();
    let fs = MemFs::new();
    proof_settings(&env, &fs, "enforce");
    let signer = SigningFixture::new()?;
    fs.insert("/etc/aauth/jwks.json", signer.jwks_document());
    env.set("BRIDGE_CONNECT_TIMEOUT_SECS", "5");
    let callout = callout_xkey()?;
    env.set(ENV_AUTH_CALLOUT_XKEY_PUBLIC, callout.public_key().as_str());
    let nats = NatsConfig::from_url(server.address()).with_tls(NatsTls::Disabled);
    let refused = bootstrap_nats_transport(
        &env,
        &nats,
        resolve_a2a_prefix(&env)?,
        EdgeAuthConfig::from_env(&env, &fs).map_err(BootstrapError::EdgeAuth)?,
    )
    .await;
    assert!(matches!(
        refused,
        Err(BootstrapError::ReplayBucket(ReplayBucketError::Open(_)))
    ));

    server
        .jetstream()
        .await
        .create_key_value(kv::Config {
            bucket: AAUTH_REPLAY_BUCKET.to_owned(),
            history: ReplayBucketSpec::REQUIRED.history(),
            max_age: ReplayBucketSpec::REQUIRED.max_age(),
            storage: StorageType::File,
            ..Default::default()
        })
        .await?;
    let state = bootstrap_nats_transport(
        &env,
        &nats,
        resolve_a2a_prefix(&env)?,
        EdgeAuthConfig::from_env(&env, &fs).map_err(BootstrapError::EdgeAuth)?,
    )
    .await?;
    let app = gateway_router(state);
    let response = app.clone().oneshot(rpc_request()?).await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let mut mint = client.subscribe("a2a.bridge.auth.callout.request").await?;
    let mut gateway = client.subscribe("a2a.v1.gateway.planner.>").await?;
    client.flush().await?;
    let minted = StubAuthCalloutMint::fixture()?
        .mint(&CallerHttpsAuth::new("Bearer fixture"))
        .await?;
    let mint_reply = Bytes::from(serde_json::to_vec(&BridgeMintResponse {
        user_jwt: minted.as_str().to_owned(),
    })?);
    let gateway_reply = Bytes::from_static(br#"{"jsonrpc":"2.0","id":"bootstrap-request","result":{}}"#);
    let handle_request = app.oneshot(signer.request()?);
    let serve_wire = async {
        answer_next_request(&client, &mut mint, mint_reply).await?;
        answer_next_request(&client, &mut gateway, gateway_reply.clone()).await?;
        Ok::<(), TestError>(())
    };
    let (response, wire) = tokio::join!(handle_request, serve_wire);
    wire?;
    let response = response?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(to_bytes(response.into_body(), usize::MAX).await?, gateway_reply);
    Ok(())
}

#[tokio::test]
async fn nats_shadow_bootstrap_falls_back_to_local_replay_when_the_bucket_is_absent() -> Result<(), TestError> {
    let events = CapturedEvents::new();
    let _guard = events.install(LevelFilter::WARN);
    let server = JetStreamTestServer::start().await;
    let env = InMemoryEnv::new();
    let fs = MemFs::new();
    proof_settings(&env, &fs, "shadow");
    env.set(ENV_AAUTH_DEV_UNENFORCED, "true");
    env.set("BRIDGE_CONNECT_TIMEOUT_SECS", "5");
    env.set(ENV_AUTH_CALLOUT_XKEY_PUBLIC, callout_xkey()?.public_key().as_str());
    let nats = NatsConfig::from_url(server.address()).with_tls(NatsTls::Disabled);

    {
        let quiet = CapturedEvents::new();
        let _guard = quiet.install(LevelFilter::ERROR);
        let state = bootstrap_nats_transport(
            &env,
            &nats,
            resolve_a2a_prefix(&env)?,
            EdgeAuthConfig::from_env(&env, &fs).map_err(BootstrapError::EdgeAuth)?,
        )
        .await?;
        let response = gateway_router(state).oneshot(rpc_request()?).await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(quiet.events().is_empty());
    }

    let state = bootstrap_nats_transport(
        &env,
        &nats,
        resolve_a2a_prefix(&env)?,
        EdgeAuthConfig::from_env(&env, &fs).map_err(BootstrapError::EdgeAuth)?,
    )
    .await?;
    let response = gateway_router(state).oneshot(rpc_request()?).await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let warning = events.events().into_iter().find(|event| {
        event.message() == Some("AAuth replay records stay in this process, since the shared replay bucket is unusable")
    });
    assert!(warning.is_some(), "the NATS replay fallback is diagnosed");
    if let Some(warning) = warning {
        assert_eq!(warning.field("mode"), Some("shadow"));
        assert!(warning.field("error").is_some());
    }
    Ok(())
}

/// Opts the bridge out of enforcing proof of possession, which every test
/// that is not about the edge does to reach the part it is about.
fn unenforced(env: &InMemoryEnv) {
    env.set(ENV_AAUTH_MODE, EdgeAuthMode::Off.as_str());
    env.set(ENV_AAUTH_DEV_UNENFORCED, "true");
}

/// The least an enforcing edge needs: who it is, where it is reached, and an
/// issuer key set to check agent tokens against.
fn proof_settings(env: &InMemoryEnv, fs: &MemFs, mode: &str) {
    env.set(ENV_AAUTH_MODE, mode);
    env.set(ENV_AAUTH_RESOURCE_ISS, "https://bridge.test");
    env.set(ENV_AAUTH_AUTHORITY, "bridge.test");
    env.set(ENV_AAUTH_JWKS_PATH, "/etc/aauth/jwks.json");
    fs.insert("/etc/aauth/jwks.json", r#"{"https://agents.test":{"keys":[]}}"#);
}

#[tokio::test]
async fn stub_bootstrap_returns_an_explicit_authentication_failure() -> Result<(), TestError> {
    let env = InMemoryEnv::new();
    env.set("AUTH_CALLOUT_NATS_URL", "nats://auth:4222");
    let state = bootstrap_stub_transport(
        &env,
        &NatsConfig::from_url("nats://unused:4222"),
        resolve_a2a_prefix(&env)?,
        EdgeAuthConfig::Off,
    )?;
    let response = gateway_router(state).oneshot(rpc_request()?).await?;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let payload: serde_json::Value = serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await?)?;
    assert_eq!(
        payload["error"],
        "auth callout mint failed: auth callout not configured for this deployment"
    );
    Ok(())
}

#[test]
fn callout_xkey_input_is_optional_but_must_be_a_curve_public_key_when_set() -> Result<(), TestError> {
    let env = InMemoryEnv::new();
    assert!(resolve_callout_xkey(&env)?.is_none());
    env.set(ENV_AUTH_CALLOUT_XKEY_PUBLIC, "   ");
    assert!(resolve_callout_xkey(&env)?.is_none());
    let callout = callout_xkey()?;
    env.set(
        ENV_AUTH_CALLOUT_XKEY_PUBLIC,
        format!(" {} ", callout.public_key().as_str()),
    );
    assert_eq!(resolve_callout_xkey(&env)?, Some(callout.public_key()));
    env.set(ENV_AUTH_CALLOUT_XKEY_PUBLIC, "not-an-xkey");
    assert!(matches!(
        resolve_callout_xkey(&env),
        Err(BootstrapError::CalloutXkey(_))
    ));
    Ok(())
}

#[tokio::test]
async fn nats_bootstrap_refuses_a_malformed_callout_xkey_before_connecting() -> Result<(), TestError> {
    let env = InMemoryEnv::new();
    env.set(ENV_AUTH_CALLOUT_XKEY_PUBLIC, "not-an-xkey");
    let result = bootstrap_nats_transport(
        &env,
        &NatsConfig::from_url("invalid://endpoint"),
        resolve_a2a_prefix(&env)?,
        EdgeAuthConfig::Off,
    )
    .await;
    assert!(matches!(result, Err(BootstrapError::CalloutXkey(_))));
    Ok(())
}

#[tokio::test]
async fn nats_bootstrap_reports_connection_failure() -> Result<(), TestError> {
    let env = InMemoryEnv::new();
    env.set("BRIDGE_CONNECT_TIMEOUT_SECS", "0");
    let result = bootstrap_nats_transport(
        &env,
        &NatsConfig::from_url("invalid://endpoint"),
        resolve_a2a_prefix(&env)?,
        EdgeAuthConfig::Off,
    )
    .await;
    assert!(matches!(result, Err(BootstrapError::NatsConnect(_))));
    Ok(())
}

#[tokio::test]
async fn bootstrap_diagnostics_reach_the_log_facade_before_tracing_initialization() -> Result<(), TestError> {
    let Some(logs) = CapturedLogs::isolated() else {
        return Ok(());
    };
    let env = InMemoryEnv::new();
    env.set(ENV_A2A_PREFIX, "tenant");
    env.set("BRIDGE_CONNECT_TIMEOUT_SECS", "2");
    env.set("BRIDGE_AUTH_MINT_TIMEOUT_SECS", "3");
    env.set("BRIDGE_GATEWAY_RPC_TIMEOUT_SECS", "4");
    env.set("NATS_URL", " nats://stub:4222 ");
    let _stub = bootstrap_stub_transport(
        &env,
        &NatsConfig::from_env(&env),
        resolve_a2a_prefix(&env)?,
        EdgeAuthConfig::Off,
    )?;
    let server = CoreTestServer::start().await;
    let _state = bootstrap_nats_transport(
        &env,
        &NatsConfig::from_url(server.address()).with_tls(NatsTls::Disabled),
        resolve_a2a_prefix(&env)?,
        EdgeAuthConfig::Off,
    )
    .await?;

    let records = logs.records();
    let stub = records
        .iter()
        .find(|record| record.message.contains("using stub transports"));
    assert!(stub.is_some_and(|record| record.level == LogLevel::Warn
        && record.message.contains("nats_url=nats://stub:4222")
        && record.message.contains("prefix=tenant")));
    let unsealed = records
        .iter()
        .find(|record| record.message.contains("no callout xkey is configured"));
    assert!(
        unsealed.is_some_and(
            |record| record.level == LogLevel::Warn && record.message.contains(ENV_AUTH_CALLOUT_XKEY_PUBLIC)
        )
    );
    let nats = records
        .iter()
        .find(|record| record.message.contains("a2a-bridge NATS transports wired"));
    assert!(nats.is_some_and(|record| record.level == LogLevel::Info
        && record.message.contains(server.address())
        && record.message.contains("connect_timeout_secs=2")
        && record.message.contains("mint_wire_secs=3")
        && record.message.contains("gateway_rpc_secs=4")
        && record.message.contains("prefix=tenant")));
    Ok(())
}

#[tokio::test]
async fn nats_bootstrap_wires_mint_subject_tenant_and_gateway_namespace() -> Result<(), TestError> {
    let Some(logs) = CapturedLogs::isolated() else {
        return Ok(());
    };
    let server = CoreTestServer::start().await;
    let client = trogon_nats::connect(
        &NatsConfig::from_url(server.address()).with_tls(NatsTls::Disabled),
        Duration::from_secs(5),
    )
    .await?;
    let callout = callout_xkey()?;
    let caller = CallerHttpsAuth::new("Bearer fixture");
    let jwt = StubAuthCalloutMint::fixture()?.mint(&caller).await?;
    let mint_reply = serde_json::to_vec(&BridgeMintResponse {
        user_jwt: jwt.as_str().to_owned(),
    })?;
    let expected = Bytes::from_static(br#"{"jsonrpc":"2.0","id":"bootstrap-request","result":{}}"#);
    for (configured_subject, expected_subject, account) in [
        (None, "a2a.bridge.auth.callout.request", None),
        (Some("  custom.mint  "), "custom.mint", Some("tenant-account")),
        (Some("   "), "a2a.bridge.auth.callout.request", Some("")),
    ] {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", server.address());
        env.set("NATS_TLS_DISABLED", "true");
        env.set(ENV_A2A_PREFIX, "tenant");
        env.set("BRIDGE_CONNECT_TIMEOUT_SECS", "2");
        env.set("BRIDGE_AUTH_MINT_TIMEOUT_SECS", "10");
        env.set("BRIDGE_GATEWAY_RPC_TIMEOUT_SECS", "2");
        env.set(ENV_AUTH_CALLOUT_XKEY_PUBLIC, callout.public_key().as_str());
        if configured_subject == Some("  custom.mint  ") {
            env.set("NATS_USER", "fixture");
            env.set("NATS_PASSWORD", "fixture");
        }
        if let Some(subject) = configured_subject {
            env.set("AUTH_CALLOUT_MINT_SUBJECT", subject);
        }
        if let Some(account) = account {
            env.set("BRIDGE_TENANT_ACCOUNT", account);
        }
        let mut mint = client.subscribe(expected_subject).await?;
        let mut gateway = client
            .subscribe("tenant.v1.gateway.planner.person.harness.message.send")
            .await?;
        client.flush().await?;
        let state = bootstrap_nats_transport(
            &env,
            &NatsConfig::from_env(&env),
            resolve_a2a_prefix(&env)?,
            EdgeAuthConfig::Off,
        )
        .await?;
        let handle_request = gateway_router(state).oneshot(rpc_request()?);
        let serve_wire = async {
            let request = tokio::time::timeout(Duration::from_secs(5), mint.next())
                .await?
                .ok_or(TestError::MissingMessage)?;
            let envelope: BridgeMintRequest = serde_json::from_slice(&request.payload)?;
            let sealed = envelope.sealed_credential.ok_or(TestError::MissingSealedCredential)?;
            assert!(matches!(
                sealed.open(&callout)?,
                BridgeCredential::BearerToken(token) if token.as_str() == "caller-fixture"
            ));
            assert_eq!(envelope.account.as_deref(), account.filter(|value| !value.is_empty()));
            client
                .publish(
                    request.reply.ok_or(TestError::MissingMessage)?,
                    mint_reply.clone().into(),
                )
                .await?;
            let request = tokio::time::timeout(Duration::from_secs(5), gateway.next())
                .await?
                .ok_or(TestError::MissingMessage)?;
            let payload: serde_json::Value = serde_json::from_slice(&request.payload)?;
            assert_eq!(payload["id"], "bootstrap-request");
            assert_eq!(
                request
                    .headers
                    .and_then(|headers| headers.get("Trogon-Req-Id").map(ToString::to_string))
                    .as_deref(),
                Some("bootstrap-request")
            );
            client
                .publish(request.reply.ok_or(TestError::MissingMessage)?, expected.clone())
                .await?;
            Ok::<(), TestError>(())
        };
        let (response, wire) = tokio::join!(handle_request, serve_wire);
        wire?;
        let response = response?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(to_bytes(response.into_body(), usize::MAX).await?, expected);
        mint.unsubscribe().await?;
        gateway.unsubscribe().await?;
    }
    assert!(logs.records().iter().any(|record| {
        record.message.contains("a2a-bridge NATS transports wired")
            && record.message.contains("prefix=tenant")
            && record.message.contains("connect_timeout_secs=2")
            && record.message.contains("mint_wire_secs=10")
            && record.message.contains("gateway_rpc_secs=2")
    }));
    Ok(())
}

#[tokio::test]
async fn nats_bootstrap_bounds_caller_connections_by_the_connect_timeout_not_the_gateway_rpc_timeout()
-> Result<(), TestError> {
    // The first thread to reach a log callsite fixes its interest for the whole process, so a thread
    // without a subscriber would silence it for the tests that capture those logs.
    let _logs = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_writer(std::io::sink)
            .finish(),
    );
    let server = CoreTestServer::start().await;
    let client = trogon_nats::connect(
        &NatsConfig::from_url(server.address()).with_tls(NatsTls::Disabled),
        Duration::from_secs(5),
    )
    .await?;
    let jwt = StubAuthCalloutMint::fixture()?
        .mint(&CallerHttpsAuth::new("Bearer fixture"))
        .await?;
    let mint_reply = Bytes::from(serde_json::to_vec(&BridgeMintResponse {
        user_jwt: jwt.as_str().to_owned(),
    })?);
    let gateway_reply = Bytes::from_static(br#"{"jsonrpc":"2.0","id":"bootstrap-request","result":{}}"#);
    let mut mint = client.subscribe("a2a.bridge.auth.callout.request").await?;
    let mut gateway = client.subscribe("a2a.v1.gateway.planner.>").await?;
    client.flush().await?;
    let callout = callout_xkey()?;

    for (body, relayed_connections, failure) in [
        (
            r#"{"jsonrpc":"2.0","id":"bootstrap-request","method":"message/send","params":{}}"#,
            1,
            "NATS gateway publish failed",
        ),
        (
            r#"{"jsonrpc":"2.0","id":"bootstrap-request","method":"tasks/resubscribe","params":{"id":"task-1"}}"#,
            2,
            "JetStream SSE consumer attach failed",
        ),
    ] {
        let proxy = StallingProxy::start(server.address(), relayed_connections).await?;
        let env = InMemoryEnv::new();
        env.set("NATS_URL", &proxy.address);
        env.set("NATS_TLS_DISABLED", "true");
        env.set(ENV_AUTH_CALLOUT_XKEY_PUBLIC, callout.public_key().as_str());
        env.set("BRIDGE_CONNECT_TIMEOUT_SECS", "2");
        env.set("BRIDGE_AUTH_MINT_TIMEOUT_SECS", "10");
        env.set("BRIDGE_GATEWAY_RPC_TIMEOUT_SECS", "3600");
        let state = bootstrap_nats_transport(
            &env,
            &NatsConfig::from_env(&env),
            resolve_a2a_prefix(&env)?,
            EdgeAuthConfig::Off,
        )
        .await?;

        let handle_request = tokio::time::timeout(
            Duration::from_secs(30),
            gateway_router(state).oneshot(rpc_request_with(body)?),
        );
        let serve_wire = async {
            answer_next_request(&client, &mut mint, mint_reply.clone()).await?;
            if relayed_connections > 1 {
                answer_next_request(&client, &mut gateway, gateway_reply.clone()).await?;
            }
            Ok::<(), TestError>(())
        };
        let (response, wire) = tokio::join!(handle_request, serve_wire);
        wire?;
        let response = response??;

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let payload: serde_json::Value = serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await?)?;
        assert!(
            payload["error"]
                .as_str()
                .is_some_and(|message| message.starts_with(failure)),
            "{payload}"
        );
    }
    Ok(())
}
