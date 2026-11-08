//! Signs requests the way an AAuth agent does, so the edge's tests run the
//! real verifier rather than a stand-in for it.

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Uri};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use bytes::Bytes;
use jsonwebtoken::jwk::{
    AlgorithmParameters, CommonParameters, EllipticCurve, EllipticCurveKeyParameters, EllipticCurveKeyType, Jwk,
    JwkSet, PublicKeyUse,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use p256::ecdsa::SigningKey;
use p256::pkcs8::LineEnding;
use pkcs8::EncodePrivateKey as _;
use rand_core::OsRng;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use trogon_aauth_verify::constants::{SERVER_NONCE_COMPONENT, SERVER_NONCE_HEADER};
use trogon_aauth_verify::{ReplayError, ReplayStore, StaticJwks, SystemTimeSource, TimeSource};
use trogon_identity_types::aauth::TYP_AGENT;
use trogon_identity_types::aauth::headers::{CONTENT_DIGEST, SIGNATURE, SIGNATURE_INPUT, SIGNATURE_KEY};
use trogon_std::env::InMemoryEnv;
use trogon_std::fs::MemFs;

use crate::constants::{
    ENV_AAUTH_AUTHORITY, ENV_AAUTH_CHALLENGE_KEY_PATH, ENV_AAUTH_CHALLENGE_KID, ENV_AAUTH_JWKS_PATH, ENV_AAUTH_NONCE,
    ENV_AAUTH_RESOURCE_ISS,
};
use crate::identity::CallerHttpsAuth;
use crate::request::InboundRequest;

pub(crate) const AGENT_ISSUER: &str = "https://agents.test";
pub(crate) const AGENT_SUBJECT: &str = "aauth:agent-1@agents.test";
pub(crate) const EDGE_AUTHORITY: &str = "bridge.test";
pub(crate) const EDGE_RESOURCE: &str = "https://bridge.test";
pub(crate) const REQUEST_PATH: &str = "/";
pub(crate) const JWKS_PATH: &str = "/etc/aauth/jwks.json";
pub(crate) const CHALLENGE_KEY_PATH: &str = "/etc/aauth/challenge.pem";
pub(crate) const CHALLENGE_KID: &str = "edge-challenge-1";
const PROVIDER_KID: &str = "provider-1";
const COVERED: [&str; 5] = ["@method", "@authority", "@path", "signature-key", "content-digest"];

static NEXT_JTI: AtomicU64 = AtomicU64::new(0);

/// A `jti` no other request in this test binary signs under.
pub(crate) fn fresh_jti() -> String {
    format!("edge-jti-{}", NEXT_JTI.fetch_add(1, Ordering::Relaxed))
}

pub(crate) fn now() -> i64 {
    SystemTimeSource.now()
}

/// A P-256 key pair, with its public half as a JWK.
pub(crate) struct EcKey {
    encoding: EncodingKey,
    jwk: Jwk,
    jwk_json: Value,
}

impl EcKey {
    pub(crate) fn generate(kid: &str) -> Self {
        let signing = SigningKey::random(&mut OsRng);
        let point = signing.verifying_key().to_encoded_point(false);
        let x = URL_SAFE_NO_PAD.encode(point.x().expect("x coordinate"));
        let y = URL_SAFE_NO_PAD.encode(point.y().expect("y coordinate"));
        let pem = signing.to_pkcs8_pem(LineEnding::LF).expect("pkcs8 pem");
        let encoding = EncodingKey::from_ec_pem(pem.as_bytes()).expect("encoding key");
        let jwk = Jwk {
            common: CommonParameters {
                public_key_use: Some(PublicKeyUse::Signature),
                key_id: Some(kid.into()),
                ..Default::default()
            },
            algorithm: AlgorithmParameters::EllipticCurve(EllipticCurveKeyParameters {
                key_type: EllipticCurveKeyType::EC,
                curve: EllipticCurve::P256,
                x: x.clone(),
                y: y.clone(),
            }),
        };
        let jwk_json = json!({"kty": "EC", "crv": "P-256", "x": x, "y": y, "kid": kid, "alg": "ES256"});
        Self {
            encoding,
            jwk,
            jwk_json,
        }
    }
}

/// A fresh PKCS#8 PEM P-256 key, the form the edge's challenge key is
/// provisioned in.
pub(crate) fn challenge_key_pem() -> String {
    SigningKey::random(&mut OsRng)
        .to_pkcs8_pem(LineEnding::LF)
        .expect("pkcs8 pem")
        .to_string()
}

/// The signature parameters a request is signed under.
#[derive(Debug, Clone)]
pub(crate) struct Signing {
    created: i64,
    expires: i64,
    jti: String,
    authority: String,
    nonce: Option<String>,
}

impl Signing {
    /// A one-minute window opening now, for the edge's authority, under a
    /// fresh `jti`.
    pub(crate) fn fresh() -> Self {
        let created = now();
        Self {
            created,
            expires: created + 60,
            jti: fresh_jti(),
            authority: EDGE_AUTHORITY.to_owned(),
            nonce: None,
        }
    }

    pub(crate) fn window(mut self, created: i64, expires: i64) -> Self {
        self.created = created;
        self.expires = expires;
        self
    }

    pub(crate) fn jti(mut self, jti: &str) -> Self {
        jti.clone_into(&mut self.jti);
        self
    }

    pub(crate) fn authority(mut self, authority: &str) -> Self {
        authority.clone_into(&mut self.authority);
        self
    }

    pub(crate) fn server_nonce(mut self, nonce: &str) -> Self {
        self.nonce = Some(nonce.to_owned());
        self
    }
}

/// An agent whose provider publishes the key its agent token is signed with,
/// and whose own key signs each request.
pub(crate) struct SigningAgent {
    provider: EcKey,
    key: EcKey,
    issuer: String,
    subject: String,
}

impl SigningAgent {
    pub(crate) fn new() -> Self {
        Self::named(AGENT_ISSUER, AGENT_SUBJECT)
    }

    pub(crate) fn named(issuer: &str, subject: &str) -> Self {
        Self {
            provider: EcKey::generate(PROVIDER_KID),
            key: EcKey::generate("agent-key"),
            issuer: issuer.to_owned(),
            subject: subject.to_owned(),
        }
    }

    pub(crate) fn issuer(&self) -> &str {
        &self.issuer
    }

    pub(crate) fn subject(&self) -> &str {
        &self.subject
    }

    /// The provider's key set, as the edge resolves it.
    pub(crate) fn jwks(&self) -> StaticJwks {
        StaticJwks::new().with(
            self.issuer.clone(),
            JwkSet {
                keys: vec![self.provider.jwk.clone()],
            },
        )
    }

    /// The provider's key set in the file form the edge loads it from.
    pub(crate) fn jwks_document(&self) -> String {
        json!({ self.issuer.clone(): {"keys": [self.provider.jwk_json]} }).to_string()
    }

    pub(crate) fn agent_token(&self) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.typ = Some(TYP_AGENT.into());
        header.kid = Some(PROVIDER_KID.into());
        let issued = now();
        let claims = json!({
            "iss": self.issuer,
            "sub": self.subject,
            "jti": fresh_jti(),
            "iat": issued - 10,
            "exp": issued + 3600,
            "dwk": "aauth-agent.json",
            "cnf": {"jwk": self.key.jwk_json},
        });
        jsonwebtoken::encode(&header, &claims, &self.provider.encoding).expect("agent token")
    }

    /// A bearer credential naming this agent's caller.
    pub(crate) fn bearer(&self) -> CallerHttpsAuth {
        bearer_for(&self.issuer, &self.subject)
    }

    /// A POST to the edge carrying `body`, signed under `signing`.
    pub(crate) fn signed(&self, body: &'static [u8], signing: &Signing) -> InboundRequest {
        post(
            self.signature_headers(Method::POST.as_str(), REQUEST_PATH, body, signing),
            Bytes::from_static(body),
        )
    }

    /// The headers an agent adds to sign a request.
    pub(crate) fn signature_headers(&self, method: &str, path: &str, body: &[u8], signing: &Signing) -> HeaderMap {
        let signature_key = format!("sig=jwt;jwt=\"{}\"", self.agent_token());
        let digest = format!("sha-256=:{}:", STANDARD.encode(Sha256::digest(body)));
        let mut components = COVERED.to_vec();
        if signing.nonce.is_some() {
            components.push(SERVER_NONCE_COMPONENT);
        }
        let covered = components
            .iter()
            .map(|component| format!("\"{component}\""))
            .collect::<Vec<_>>()
            .join(" ");
        let params = format!(
            "({covered});created={};expires={};nonce=\"{}\"",
            signing.created, signing.expires, signing.jti
        );
        let lines = components
            .iter()
            .map(|component| {
                let value = match *component {
                    "@method" => method.to_uppercase(),
                    "@authority" => signing.authority.to_lowercase(),
                    "@path" => path.to_owned(),
                    "signature-key" => signature_key.clone(),
                    "content-digest" => digest.clone(),
                    _ => signing.nonce.clone().expect("covered nonce"),
                };
                format!("\"{component}\": {value}")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let base = format!("{lines}\n\"@signature-params\": {params}");
        let signature =
            jsonwebtoken::crypto::sign(base.as_bytes(), &self.key.encoding, Algorithm::ES256).expect("signature");
        let mut headers = HeaderMap::new();
        insert(&mut headers, SIGNATURE_KEY, &signature_key);
        insert(&mut headers, SIGNATURE_INPUT, &format!("sig={params}"));
        insert(&mut headers, SIGNATURE, &format!("sig=:{signature}:"));
        insert(&mut headers, CONTENT_DIGEST, &digest);
        if let Some(nonce) = &signing.nonce {
            insert(&mut headers, SERVER_NONCE_HEADER, nonce);
        }
        headers
    }
}

/// A bearer credential shaped like a compact JWT naming `issuer` and
/// `subject`. Its signature is never checked, since only the callout
/// verifies the bearer.
pub(crate) fn bearer_for(issuer: &str, subject: &str) -> CallerHttpsAuth {
    CallerHttpsAuth::new(format!(
        "Bearer {}",
        bearer_jwt(&json!({"iss": issuer, "sub": subject}))
    ))
}

pub(crate) fn bearer_jwt(claims: &Value) -> String {
    let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
    format!("eyJhbGciOiJub25lIn0.{payload}.sig")
}

/// A POST to the edge's route, as the route receives it.
pub(crate) fn post(headers: HeaderMap, body: Bytes) -> InboundRequest {
    InboundRequest::new(Method::POST, Uri::from_static(REQUEST_PATH), headers, body)
}

/// A request that carries no signature at all.
pub(crate) fn unsigned(body: &'static [u8]) -> InboundRequest {
    post(HeaderMap::new(), Bytes::from_static(body))
}

/// `headers` with `Authorization` set to `authorization`, the way the HTTP
/// route receives a caller's bearer.
pub(crate) fn with_authorization(mut headers: HeaderMap, authorization: &str) -> HeaderMap {
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(authorization).expect("authorization"),
    );
    headers
}

fn insert(headers: &mut HeaderMap, name: &str, value: &str) {
    headers.insert(
        HeaderName::from_bytes(name.as_bytes()).expect("header name"),
        HeaderValue::from_str(value).expect("header value"),
    );
}

/// A replay store that cannot answer, as when its bucket is unreachable.
pub(crate) struct FailingReplayStore;

#[async_trait]
impl ReplayStore for FailingReplayStore {
    async fn check_and_insert(&self, _key: &str, _ttl_secs: u32) -> Result<bool, ReplayError> {
        Err(ReplayError::MutexPoisoned)
    }

    async fn contains(&self, _key: &str) -> Result<bool, ReplayError> {
        Err(ReplayError::MutexPoisoned)
    }
}

/// The environment and files of an edge that verifies proofs against one
/// agent's provider keys, read from a key sets file.
pub(crate) struct EdgeEnv {
    env: InMemoryEnv,
    fs: MemFs,
}

impl EdgeEnv {
    pub(crate) fn trusting(agent: &SigningAgent) -> Self {
        let env = InMemoryEnv::new();
        env.set(ENV_AAUTH_RESOURCE_ISS, EDGE_RESOURCE);
        env.set(ENV_AAUTH_AUTHORITY, EDGE_AUTHORITY);
        env.set(ENV_AAUTH_JWKS_PATH, JWKS_PATH);
        let fs = MemFs::new();
        fs.insert(JWKS_PATH, agent.jwks_document());
        Self { env, fs }
    }

    pub(crate) fn set(self, var: &str, value: &str) -> Self {
        self.env.set(var, value);
        self
    }

    pub(crate) fn unset(self, var: &str) -> Self {
        self.env.remove(var);
        self
    }

    pub(crate) fn file(self, path: &str, content: &str) -> Self {
        self.fs.insert(path, content);
        self
    }

    /// Demands a server nonce on every request, signed with a fresh
    /// challenge key.
    pub(crate) fn requiring_nonces(self) -> Self {
        self.set(ENV_AAUTH_NONCE, "required")
            .set(ENV_AAUTH_CHALLENGE_KEY_PATH, CHALLENGE_KEY_PATH)
            .set(ENV_AAUTH_CHALLENGE_KID, CHALLENGE_KID)
            .file(CHALLENGE_KEY_PATH, &challenge_key_pem())
    }

    pub(crate) fn env(&self) -> &InMemoryEnv {
        &self.env
    }

    pub(crate) fn fs(&self) -> &MemFs {
        &self.fs
    }
}
