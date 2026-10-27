use axum::{Json, Router, extract::State, routing::get};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, encode, jwk::Jwk};
use mcp_postgres::{AuthError, AuthenticationConfigInput, Authenticator};
use rsa::{RsaPrivateKey, pkcs8::EncodePrivateKey, rand_core::OsRng};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Clone)]
struct StateData {
    document: Arc<RwLock<Value>>,
    requests: Arc<AtomicUsize>,
}

struct Fixture {
    issuer: String,
    key: EncodingKey,
    state: StateData,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Fixture {
    async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let pair = rcgen::KeyPair::generate()?;
        let key = EncodingKey::from_ec_der(&pair.serialize_der());
        let mut jwk = Jwk::from_encoding_key(&key, Algorithm::ES256)?;
        jwk.common.key_id = Some("key-one".to_owned());
        let state = StateData {
            document: Arc::new(RwLock::new(json!({"keys":[jwk]}))),
            requests: Arc::new(AtomicUsize::new(0)),
        };
        let app = Router::new()
            .route(
                "/jwks",
                get(|State(state): State<StateData>| async move {
                    state.requests.fetch_add(1, Ordering::SeqCst);
                    Json(state.document.read().await.clone())
                }),
            )
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let issuer = format!("http://{}", listener.local_addr()?);
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self {
            issuer,
            key,
            state,
            task,
        })
    }

    fn config(&self) -> Value {
        json!({"issuers":[{
            "issuer":self.issuer,"jwks_uri":format!("{}/jwks",self.issuer),"audience":"sql-mcp",
            "required_scopes":["sql:read"],
            "users":{"external-user":"user-one"},
            "clients":{"external-client":"client-one","agent":"agent-one","portal":"portal-one"}
        }]})
    }

    fn claims(&self) -> Value {
        json!({"iss":self.issuer,"aud":"sql-mcp","exp":now()+300,"sub":"external-user",
            "client_id":"external-client","scope":"sql:read"})
    }

    fn token(&self, claims: &Value) -> Result<String, jsonwebtoken::errors::Error> {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("key-one".to_owned());
        encode(&header, claims, &self.key)
    }

    async fn auth(&self) -> Result<Authenticator, Box<dyn std::error::Error>> {
        let input: AuthenticationConfigInput = serde_json::from_value(self.config())?;
        Ok(Authenticator::new(input).await?)
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_secs())
}

#[tokio::test]
async fn maps_verified_issuer_identity_and_delegated_actor() -> TestResult {
    let fixture = Fixture::new().await?;
    let auth = fixture.auth().await?;
    let direct = auth.authenticate(&fixture.token(&fixture.claims())?).await?;
    assert_eq!(direct.user().as_str(), "user-one");
    assert_eq!(direct.client().as_str(), "client-one");
    assert!(direct.delegation().is_empty());
    let mut delegated = fixture.claims();
    delegated["act"] = json!({"sub":"agent","act":{"sub":"portal"}});
    let principal = auth.authenticate(&fixture.token(&delegated)?).await?;
    assert_eq!(principal.client().as_str(), "agent-one");
    assert_eq!(
        principal
            .delegation()
            .iter()
            .map(|actor| actor.as_str())
            .collect::<Vec<_>>(),
        ["agent-one", "portal-one"]
    );
    assert_eq!(auth.issuer_urls(), std::slice::from_ref(&fixture.issuer));
    assert_eq!(fixture.state.requests.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn maps_configured_okta_claims_without_using_email_subject_as_local_identity() -> TestResult {
    let fixture = Fixture::new().await?;
    let mut config = fixture.config();
    config["issuers"][0]["user_claim"] = json!("uid");
    config["issuers"][0]["client_claim"] = json!("cid");
    config["issuers"][0]["scope_claim"] = json!("scp");
    config["issuers"][0]["users"] = json!({"00u-test-user":"user-one"});
    config["issuers"][0]["clients"] = json!({"0oa-test-client":"client-one"});
    let auth = Authenticator::new(serde_json::from_value(config)?).await?;
    let mut claims = json!({
        "iss":fixture.issuer,"aud":"sql-mcp","exp":now()+300,
        "sub":"reader@example.test","uid":"00u-test-user","cid":"0oa-test-client",
        "scp":["openid","sql:read"]
    });
    let identity = auth.authenticate(&fixture.token(&claims)?).await?;
    assert_eq!(identity.user().as_str(), "user-one");
    assert_eq!(identity.client().as_str(), "client-one");
    claims["uid"] = json!("unmapped-user");
    assert!(matches!(
        auth.authenticate(&fixture.token(&claims)?).await,
        Err(AuthError::Unauthenticated)
    ));
    claims["uid"] = json!("00u-test-user");
    claims["scp"] = json!(["openid"]);
    assert!(matches!(
        auth.authenticate(&fixture.token(&claims)?).await,
        Err(AuthError::InsufficientScope)
    ));
    Ok(())
}

#[tokio::test]
async fn verifies_ephemeral_rsa_signed_access_token_against_published_jwks() -> TestResult {
    let fixture = Fixture::new().await?;
    let private = tokio::task::spawn_blocking(|| RsaPrivateKey::new(&mut OsRng, 2048)).await??;
    let pem = private.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)?;
    let key = EncodingKey::from_rsa_pem(pem.as_bytes())?;
    let mut jwk = Jwk::from_encoding_key(&key, Algorithm::RS256)?;
    jwk.common.key_id = Some("rsa-key".to_owned());
    *fixture.state.document.write().await = json!({"keys":[jwk]});
    let auth = fixture.auth().await?;
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("rsa-key".to_owned());
    let token = encode(&header, &fixture.claims(), &key)?;
    let identity = auth.authenticate(&token).await?;
    assert_eq!(identity.user().as_str(), "user-one");
    assert_eq!(identity.client().as_str(), "client-one");
    assert_eq!(fixture.state.requests.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn verifies_ephemeral_ed25519_signed_access_token_against_published_jwks() -> TestResult {
    let fixture = Fixture::new().await?;
    let pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519)?;
    let key = EncodingKey::from_ed_der(&pair.serialize_der());
    let public = DecodingKey::from_ed_der(pair.public_key_raw());
    let mut jwk = Jwk::from_decoding_key(&public, Some(Algorithm::EdDSA))?;
    jwk.common.key_id = Some("ed25519-key".to_owned());
    *fixture.state.document.write().await = json!({"keys":[jwk]});
    let auth = fixture.auth().await?;
    let mut header = Header::new(Algorithm::EdDSA);
    header.kid = Some("ed25519-key".to_owned());
    let token = encode(&header, &fixture.claims(), &key)?;
    let identity = auth.authenticate(&token).await?;
    assert_eq!(identity.user().as_str(), "user-one");
    assert_eq!(identity.client().as_str(), "client-one");
    Ok(())
}

#[tokio::test]
async fn rejects_invalid_claims_signatures_and_scopes() -> TestResult {
    let fixture = Fixture::new().await?;
    let auth = fixture.auth().await?;
    for (claim, value) in [
        ("iss", json!("https://untrusted.example.test")),
        ("aud", json!("other")),
        ("exp", json!(now() - 1)),
        ("nbf", json!(now() + 300)),
        ("sub", json!("")),
        ("sub", json!("unknown-user")),
        ("client_id", json!("unknown-client")),
        ("chain_act", json!([])),
        ("act", json!({})),
        ("act", json!({"sub":"unknown"})),
        ("act", json!({"sub":"agent","act":{"sub":"agent"}})),
        ("act", json!({"agent":"agent"})),
    ] {
        let mut claims = fixture.claims();
        claims[claim] = value;
        assert!(matches!(
            auth.authenticate(&fixture.token(&claims)?).await,
            Err(AuthError::Unauthenticated)
        ));
    }
    for claim in ["exp", "iss", "aud", "sub", "client_id"] {
        let mut claims = fixture.claims();
        claims.as_object_mut().ok_or("claims object")?.remove(claim);
        assert!(matches!(
            auth.authenticate(&fixture.token(&claims)?).await,
            Err(AuthError::Unauthenticated)
        ));
    }
    let mut claims = fixture.claims();
    claims["scope"] = json!("other:scope");
    assert!(matches!(
        auth.authenticate(&fixture.token(&claims)?).await,
        Err(AuthError::InsufficientScope)
    ));
    let other_key = rcgen::KeyPair::generate()?;
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("key-one".to_owned());
    let wrong_signature = encode(
        &header,
        &fixture.claims(),
        &EncodingKey::from_ec_der(&other_key.serialize_der()),
    )?;
    assert!(matches!(
        auth.authenticate(&wrong_signature).await,
        Err(AuthError::Unauthenticated)
    ));
    assert!(matches!(
        auth.authenticate(&"a".repeat(16385)).await,
        Err(AuthError::Unauthenticated)
    ));
    assert_eq!(fixture.state.requests.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn refreshes_rotated_keys_once_and_limits_unknown_key_fetches() -> TestResult {
    let fixture = Fixture::new().await?;
    let auth = fixture.auth().await?;
    let rotated_pair = rcgen::KeyPair::generate()?;
    let rotated_key = EncodingKey::from_ec_der(&rotated_pair.serialize_der());
    let mut jwk = Jwk::from_encoding_key(&rotated_key, Algorithm::ES256)?;
    jwk.common.key_id = Some("key-two".to_owned());
    *fixture.state.document.write().await = json!({"keys":[jwk]});
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("key-two".to_owned());
    let token = encode(&header, &fixture.claims(), &rotated_key)?;
    assert!(auth.authenticate(&token).await.is_ok());
    for kid in ["unknown-one", "unknown-two"] {
        header.kid = Some(kid.to_owned());
        let token = encode(&header, &fixture.claims(), &rotated_key)?;
        assert!(matches!(
            auth.authenticate(&token).await,
            Err(AuthError::Unauthenticated)
        ));
    }
    assert_eq!(fixture.state.requests.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn rejects_disallowed_delegation_and_unsafe_configuration() -> TestResult {
    let fixture = Fixture::new().await?;
    for (policy, act) in [("required", None), ("forbidden", Some(json!({"sub":"agent"})))] {
        let mut config = fixture.config();
        config["issuers"][0]["delegation"] = json!(policy);
        let auth = Authenticator::new(serde_json::from_value(config)?).await?;
        let mut claims = fixture.claims();
        if let Some(act) = act {
            claims["act"] = act;
        }
        assert!(matches!(
            auth.authenticate(&fixture.token(&claims)?).await,
            Err(AuthError::Unauthenticated)
        ));
    }
    for (field, value) in [
        ("jwks_uri", "http://remote.example.test/jwks"),
        ("issuer", "http://remote.example.test"),
        ("audience", ""),
    ] {
        let mut config = fixture.config();
        config["issuers"][0][field] = json!(value);
        assert!(matches!(
            Authenticator::new(serde_json::from_value(config)?).await,
            Err(AuthError::Configuration)
        ));
    }
    Ok(())
}

#[tokio::test]
async fn rejects_incompatible_and_ambiguous_jwk_metadata() -> TestResult {
    let fixture = Fixture::new().await?;
    for (field, value) in [
        ("use", json!("enc")),
        ("key_ops", json!(["encrypt"])),
        ("alg", json!("RS256")),
    ] {
        let mut jwk = serde_json::to_value(Jwk::from_encoding_key(&fixture.key, Algorithm::ES256)?)?;
        jwk["kid"] = json!("key-one");
        jwk[field] = value;
        *fixture.state.document.write().await = json!({"keys":[jwk]});
        let auth = fixture.auth().await?;
        assert!(matches!(
            auth.authenticate(&fixture.token(&fixture.claims())?).await,
            Err(AuthError::Unauthenticated)
        ));
    }
    let mut jwk = serde_json::to_value(Jwk::from_encoding_key(&fixture.key, Algorithm::ES256)?)?;
    jwk["kid"] = json!("key-one");
    *fixture.state.document.write().await = json!({"keys":[jwk.clone(),jwk]});
    let auth = fixture.auth().await?;
    assert!(matches!(
        auth.authenticate(&fixture.token(&fixture.claims())?).await,
        Err(AuthError::Unauthenticated)
    ));
    Ok(())
}

#[tokio::test]
async fn rejects_delegation_from_a_different_actor_issuer() -> TestResult {
    let fixture = Fixture::new().await?;
    let auth = fixture.auth().await?;
    for actor in [
        json!({"sub":"agent","iss":"https://other.example.test"}),
        json!({"sub":"agent","iss":1}),
        json!({"sub":"agent","act":{"sub":"portal","iss":"https://other.example.test"}}),
    ] {
        let mut claims = fixture.claims();
        claims["act"] = actor;
        assert!(matches!(
            auth.authenticate(&fixture.token(&claims)?).await,
            Err(AuthError::Unauthenticated)
        ));
    }
    let mut claims = fixture.claims();
    claims["act"] = json!({"sub":"agent","iss":fixture.issuer});
    assert!(auth.authenticate(&fixture.token(&claims)?).await.is_ok());
    Ok(())
}

#[tokio::test]
async fn refuses_unsupported_token_headers_and_malformed_tokens() -> TestResult {
    let fixture = Fixture::new().await?;
    let auth = fixture.auth().await?;
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("key-one".to_owned());
    header.crit = Some(vec!["unrecognized".to_owned()]);
    let token = encode(&header, &fixture.claims(), &fixture.key)?;
    assert!(matches!(
        auth.authenticate(&token).await,
        Err(AuthError::Unauthenticated)
    ));
    header.crit = None;
    header.kid = None;
    let token = encode(&header, &fixture.claims(), &fixture.key)?;
    assert!(matches!(
        auth.authenticate(&token).await,
        Err(AuthError::Unauthenticated)
    ));
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("key-one".to_owned());
    let token = encode(
        &header,
        &fixture.claims(),
        &EncodingKey::from_secret(b"test-only-signing-material"),
    )?;
    assert!(matches!(
        auth.authenticate(&token).await,
        Err(AuthError::Unauthenticated)
    ));
    assert!(matches!(
        auth.authenticate("not-a-jwt").await,
        Err(AuthError::Unauthenticated)
    ));
    let mut config = fixture.config();
    let duplicate = config["issuers"][0].clone();
    config["issuers"].as_array_mut().ok_or("issuer array")?.push(duplicate);
    assert!(matches!(
        Authenticator::new(serde_json::from_value(config)?).await,
        Err(AuthError::Configuration)
    ));
    Ok(())
}
