use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::*;
use crate::freshness::SignatureValidity;
use crate::jwks::StaticJwks;
use crate::replay::InMemoryReplayStore;
use crate::server_nonce::NonceTtl;
use crate::test_support::{
    EcFixture, challenge_key_from, challenge_key_pem, ed25519_fixture, jwks_with_key, p256_fixture, p384_fixture,
};
use async_trait::async_trait;
use jsonwebtoken::crypto::sign;
use jsonwebtoken::jwk::JwkSet;
use trogon_identity_types::aauth::{TYP_AGENT, TYP_AUTH};

#[derive(Clone, Copy)]
struct FixedClock(i64);

impl TimeSource for FixedClock {
    fn now(&self) -> i64 {
        self.0
    }
}

struct FailingReplayStore;

#[async_trait]
impl ReplayStore for FailingReplayStore {
    async fn check_and_insert(&self, _key: &str, _ttl_secs: u32) -> Result<bool, ReplayError> {
        Err(ReplayError::MutexPoisoned)
    }

    async fn contains(&self, _key: &str) -> Result<bool, ReplayError> {
        Err(ReplayError::MutexPoisoned)
    }
}

/// Records every `jti`, but cannot look up whether a nonce was issued.
#[derive(Default)]
struct LookupFailingReplayStore(InMemoryReplayStore);

#[async_trait]
impl ReplayStore for LookupFailingReplayStore {
    async fn check_and_insert(&self, key: &str, ttl_secs: u32) -> Result<bool, ReplayError> {
        self.0.check_and_insert(key, ttl_secs).await
    }

    async fn contains(&self, _key: &str) -> Result<bool, ReplayError> {
        Err(ReplayError::MutexPoisoned)
    }
}

/// Admits every `jti`, but finds every nonce record already taken.
struct CollidingNonceStore;

#[async_trait]
impl ReplayStore for CollidingNonceStore {
    async fn check_and_insert(&self, key: &str, _ttl_secs: u32) -> Result<bool, ReplayError> {
        Ok(!key.starts_with("nonce."))
    }

    async fn contains(&self, _key: &str) -> Result<bool, ReplayError> {
        Ok(false)
    }
}

#[derive(Default)]
struct RecordingReplayStore {
    records: Mutex<Vec<(String, u32)>>,
}

#[async_trait]
impl ReplayStore for RecordingReplayStore {
    async fn check_and_insert(&self, key: &str, ttl_secs: u32) -> Result<bool, ReplayError> {
        self.records
            .lock()
            .expect("records lock")
            .push((key.to_string(), ttl_secs));
        Ok(true)
    }

    async fn contains(&self, key: &str) -> Result<bool, ReplayError> {
        Ok(self
            .records
            .lock()
            .expect("records lock")
            .iter()
            .any(|(recorded, _)| recorded == key))
    }
}

fn verifier_with<S: ReplayStore>(jwks: StaticJwks, now: i64, replay: S) -> HttpPopVerifier<StaticJwks, FixedClock, S> {
    HttpPopVerifier::new(jwks, FixedClock(now), replay, "resource.example")
}

fn verifier_at(
    jwks: StaticJwks,
    now: i64,
    resource_identifier: &str,
) -> HttpPopVerifier<StaticJwks, FixedClock, InMemoryReplayStore> {
    HttpPopVerifier::new(
        jwks,
        FixedClock(now),
        InMemoryReplayStore::default(),
        resource_identifier,
    )
}

fn agent_jwt_expiring(fixture: &EcFixture, kid: &str, iss: &str, exp: i64) -> String {
    let mut header = jsonwebtoken::Header::new(fixture.alg);
    header.typ = Some(TYP_AGENT.into());
    header.kid = Some(kid.into());
    let claims = serde_json::json!({
        "iss": iss,
        "sub": "aauth:asst@agent.example",
        "jti": "j1",
        "iat": 1000,
        "exp": exp,
        "dwk": "aauth-agent.json",
        "cnf": {"jwk": fixture.jwk_json},
    });
    jsonwebtoken::encode(&header, &claims, &fixture.signing).expect("encode agent jwt")
}

fn agent_jwt(fixture: &EcFixture, kid: &str, iss: &str) -> String {
    agent_jwt_expiring(fixture, kid, iss, 9_999_999_999)
}

fn auth_jwt(fixture: &EcFixture, kid: &str, iss: &str, aud: &str) -> String {
    let mut header = jsonwebtoken::Header::new(fixture.alg);
    header.typ = Some(TYP_AUTH.into());
    header.kid = Some(kid.into());
    let claims = serde_json::json!({
        "iss": iss,
        "sub": "person-1",
        "aud": aud,
        "jti": "j1",
        "iat": 1000,
        "exp": 9_999_999_999_i64,
        "agent": "aauth:asst@agent.example",
        "agent_jkt": "abc",
        "scope": "read",
        "cnf": {"jwk": fixture.jwk_json},
    });
    jsonwebtoken::encode(&header, &claims, &fixture.signing).expect("encode auth jwt")
}

fn signature_key_header(jwt: &str) -> String {
    format!("sig=jwt;jwt=\"{jwt}\"")
}

static NEXT_JTI: AtomicU64 = AtomicU64::new(0);

fn fresh_jti() -> String {
    format!("jti-{}", NEXT_JTI.fetch_add(1, Ordering::Relaxed))
}

fn signature_input_with(components: &[&str], created: i64, expires: i64, jti: &str) -> String {
    let list = components
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(" ");
    format!("sig=({list});created={created};expires={expires};nonce=\"{jti}\"")
}

/// A one-minute window starting at `created`, under a jti no other request in
/// this test binary uses.
fn signature_input_header(components: &[&str], created: i64) -> String {
    signature_input_with(components, created, created + 60, &fresh_jti())
}

fn empty_body_digest() -> String {
    format!("sha-256=:{}:", STANDARD.encode(Sha256::digest(b"")))
}

fn base_request(sig_key: &str) -> HttpRequest {
    HttpRequest {
        method: "GET".to_string(),
        authority: "resource.example".to_string(),
        path: "/api/documents".to_string(),
        headers: vec![
            (headers::SIGNATURE_KEY.to_string(), sig_key.to_string()),
            (headers::CONTENT_DIGEST.to_string(), empty_body_digest()),
        ],
        body: None,
    }
}

fn replace_header(req: &mut HttpRequest, name: &str, value: String) {
    req.headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
    req.headers.push((name.to_string(), value));
}

fn remove_header(req: &mut HttpRequest, name: &str) {
    req.headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
}

/// Signs `req` with `fixture` under `sig_input`, replacing `Signature-Input`
/// and `Signature`.
fn sign_request_with_input(fixture: &EcFixture, req: &mut HttpRequest, sig_input: &str) {
    replace_header(req, headers::SIGNATURE_INPUT, sig_input.to_string());
    let parsed = SignatureInput::parse(sig_input).expect("parse for test");
    let base = build_signature_base(req, &parsed).expect("build base");
    let sig_b64 = sign(base.as_bytes(), &fixture.signing, fixture.alg).expect("sign");
    replace_header(req, headers::SIGNATURE, format!("sig=:{sig_b64}:"));
}

/// Signs `req` with `fixture` over `components` in a fresh one-minute window
/// starting at `created`.
fn sign_request(fixture: &EcFixture, req: &mut HttpRequest, created: i64, components: &[&str]) {
    sign_request_with_input(fixture, req, &signature_input_header(components, created));
}

const REQUIRED_COMPONENTS: [&str; 5] = ["@method", "@authority", "@path", "signature-key", "content-digest"];

#[tokio::test(flavor = "current_thread")]
async fn verify_accepts_agent_presenter_roundtrip() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(&fixture, &mut req, 1000, &REQUIRED_COMPONENTS);

    let result = verifier.verify(&req).await.expect("agent presenter verifies");
    assert!(matches!(result, VerifiedPresenter::Agent(_)));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_accepts_auth_presenter_roundtrip() {
    let fixture = p256_fixture("k1");
    let jwt = auth_jwt(&fixture, "k1", "as.example", "resource.example");
    let jwks = jwks_with_key("as.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(&fixture, &mut req, 1000, &REQUIRED_COMPONENTS);

    let result = verifier.verify(&req).await.expect("auth presenter verifies");
    match result {
        VerifiedPresenter::Auth(presenter) => {
            assert_eq!(presenter.auth.claims.iss, "as.example");
        }
        VerifiedPresenter::Agent(_) => panic!("expected auth presenter"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn verify_accepts_ed25519_agent_presenter() {
    let fixture = ed25519_fixture("ed-k1");
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::EdDSA);
    header.typ = Some(TYP_AGENT.into());
    header.kid = Some("ed-k1".into());
    let claims = serde_json::json!({
        "iss": "agent-provider.example",
        "sub": "aauth:asst@agent.example",
        "jti": "j1",
        "iat": 1000,
        "exp": 9_999_999_999_i64,
        "dwk": "aauth-agent.json",
        "cnf": {"jwk": fixture.jwk_json},
    });
    let jwt = jsonwebtoken::encode(&header, &claims, &fixture.encoding).expect("encode");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    let sig_input = format!("{};alg=\"ed25519\"", signature_input_header(&REQUIRED_COMPONENTS, 1000));
    replace_header(&mut req, headers::SIGNATURE_INPUT, sig_input.clone());
    let parsed = SignatureInput::parse(&sig_input).expect("parse");
    let base = build_signature_base(&req, &parsed).expect("build base");
    let sig_b64 = fixture.sign_pop_base(base.as_bytes());
    replace_header(&mut req, headers::SIGNATURE, format!("sig=:{sig_b64}:"));

    let result = verifier.verify(&req).await.expect("eddsa agent presenter verifies");
    assert!(matches!(result, VerifiedPresenter::Agent(_)));
}

/// Signs a request carrying `body` with `digest` as its covered
/// `Content-Digest`.
fn signed_body_request(fixture: &EcFixture, jwt: &str, body: &[u8], digest: String) -> HttpRequest {
    let mut req = base_request(&signature_key_header(jwt));
    req.body = Some(body.to_vec());
    replace_header(&mut req, headers::CONTENT_DIGEST, digest);
    sign_request(fixture, &mut req, 1000, &REQUIRED_COMPONENTS);
    req
}

#[tokio::test(flavor = "current_thread")]
async fn verify_accepts_body_matching_content_digest() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let body = br#"{"scope":"data.read"}"#;
    let digest = crate::nats_pop::content_digest_sha256(body);
    let req = signed_body_request(&fixture, &jwt, body, digest);

    let result = verifier.verify(&req).await.expect("matching digest verifies");
    assert!(matches!(result, VerifiedPresenter::Agent(_)));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_accepts_rfc9530_padded_base64_content_digest() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    // RFC 8941 Byte Sequences are standard padded base64, which is what a
    // conformant third-party client sends; our own emitter uses URL-safe
    // unpadded. Both encode the same digest and both must be accepted.
    let body = br#"{"scope":"data.read"}"#;
    let padded = format!("sha-256=:{}:", STANDARD.encode(Sha256::digest(body)));
    let req = signed_body_request(&fixture, &jwt, body, padded);

    verifier.verify(&req).await.expect("padded base64 digest verifies");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_tampered_body_against_covered_content_digest() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let body = br#"{"scope":"data.read"}"#;
    let digest = crate::nats_pop::content_digest_sha256(body);
    let mut req = signed_body_request(&fixture, &jwt, body, digest);

    // Swap the body after signing, leaving the covered digest header intact.
    // The signature still verifies over the untouched header, so only
    // recomputing the digest against the body catches this.
    req.body = Some(br#"{"scope":"data.write"}"#.to_vec());

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::ContentDigestMismatch));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_stripped_body_against_covered_content_digest() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let body = br#"{"scope":"data.read"}"#;
    let digest = crate::nats_pop::content_digest_sha256(body);
    let mut req = signed_body_request(&fixture, &jwt, body, digest);

    // A removed body digests as an empty one, which the covered digest does
    // not name.
    req.body = None;

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::ContentDigestMismatch));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_body_added_to_a_request_signed_as_bodyless() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(&fixture, &mut req, 1000, &REQUIRED_COMPONENTS);
    req.body = Some(br#"{"scope":"data.write"}"#.to_vec());

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::ContentDigestMismatch));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_a_bodyless_request_without_content_digest() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    // ADR#0051 Decision 2 binds every request to a digest of its body, so a
    // bodyless one still has to carry the digest of the empty body.
    let mut req = base_request(&signature_key_header(&jwt));
    remove_header(&mut req, headers::CONTENT_DIGEST);
    sign_request(
        &fixture,
        &mut req,
        1000,
        &["@method", "@authority", "@path", "signature-key"],
    );

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::MissingContentDigest));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_content_digest_left_out_of_coverage() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(
        &fixture,
        &mut req,
        1000,
        &["@method", "@authority", "@path", "signature-key"],
    );

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::MissingContentDigest));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_accepts_a_parameterized_sha256_content_digest_item() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    // RFC 8941 lets any Dictionary member's Item carry parameters. They are
    // not part of the Byte Sequence, so a peer that sends them must still
    // interoperate rather than be read as an unsupported digest.
    let body = br#"{"scope":"data.read"}"#;
    let parameterized = format!("sha-256=:{}:;q=1", STANDARD.encode(Sha256::digest(body)));
    let req = signed_body_request(&fixture, &jwt, body, parameterized);

    verifier.verify(&req).await.expect("parameterized item verifies");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_accepts_a_content_digest_carrying_a_bare_key_member() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    // RFC 8941 spells a Dictionary member with no `=` as the Boolean true, so
    // one carries no Byte Sequence to compare a body against. Reading past it
    // keeps an unknown member from displacing the `sha-256` beside it.
    let body = br#"{"scope":"data.read"}"#;
    let with_bare_key = format!("unixsum, sha-256=:{}:", STANDARD.encode(Sha256::digest(body)));
    let req = signed_body_request(&fixture, &jwt, body, with_bare_key);

    verifier.verify(&req).await.expect("a bare-key member is stepped over");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_a_bare_sha256_member_trailing_a_readable_one() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    // A trailing bare `sha-256` is the Boolean true, and last-wins hands the
    // key to it: an RFC-compliant peer is left with no digest to check. Keeping
    // the earlier Byte Sequence would let a sender show this verifier a body it
    // has agreed to and every other component nothing at all.
    let body = br#"{"scope":"data.read"}"#;
    let live = STANDARD.encode(Sha256::digest(body));
    let displaced = format!("sha-256=:{live}:, sha-256");
    let req = signed_body_request(&fixture, &jwt, body, displaced);

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::UnsupportedContentDigest));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_a_parameterized_bare_sha256_member() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    // The same displacement dressed as a parameterized member: `sha-256;q=1`
    // splits at the parameter's `=`, so the algorithm has to be read off the
    // key ahead of the `;` for the member to be recognised as the Boolean it is.
    let body = br#"{"scope":"data.read"}"#;
    let live = STANDARD.encode(Sha256::digest(body));
    let displaced = format!("sha-256=:{live}:, sha-256;q=1");
    let req = signed_body_request(&fixture, &jwt, body, displaced);

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::UnsupportedContentDigest));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_resolves_a_duplicated_sha256_member_to_the_last_one() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    // RFC 8941 Dictionary parsing keeps the last member for a repeated key.
    // Reading the first instead would let a peer show one digest to this
    // verifier and a different one to every other RFC-compliant component.
    let body = br#"{"scope":"data.read"}"#;
    let stale = STANDARD.encode(Sha256::digest(br#"{"scope":"data.write"}"#));
    let live = STANDARD.encode(Sha256::digest(body));
    let duplicated = format!("sha-256=:{stale}:, sha-256=:{live}:");
    let req = signed_body_request(&fixture, &jwt, body, duplicated);

    verifier.verify(&req).await.expect("last duplicate member wins");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_when_the_last_duplicated_sha256_member_mismatches() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    // The mirror of the case above: a correct leading member must not rescue
    // a trailing one that does not match the body.
    let body = br#"{"scope":"data.read"}"#;
    let live = STANDARD.encode(Sha256::digest(body));
    let stale = STANDARD.encode(Sha256::digest(br#"{"scope":"data.write"}"#));
    let duplicated = format!("sha-256=:{live}:, sha-256=:{stale}:");
    let req = signed_body_request(&fixture, &jwt, body, duplicated);

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::ContentDigestMismatch));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_a_readable_sha256_member_trailing_an_unreadable_one() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    // Last-wins ranges over the members this verifier can read. RFC 8941
    // discards a field carrying a member that does not parse, so a `sha-256`
    // whose Byte Sequence cannot be decoded refuses the request rather than
    // yielding to whatever follows it. Recovering would let a sender bury an
    // unreadable member under a trailing readable one.
    let body = br#"{"scope":"data.read"}"#;
    let live = STANDARD.encode(Sha256::digest(body));
    let unreadable = format!("sha-256=:not base64!:, sha-256=:{live}:");
    let req = signed_body_request(&fixture, &jwt, body, unreadable);

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::UnsupportedContentDigest));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_content_digest_without_sha256_entry() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    // Only algorithms this verifier cannot check: refused rather than
    // skipped, since skipping is indistinguishable from no body integrity.
    let body = br#"{"scope":"data.read"}"#;
    let req = signed_body_request(&fixture, &jwt, body, "sha-512=:YWJj:".to_string());

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::UnsupportedContentDigest));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_tampered_content_digest_header() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let body = br#"{"scope":"data.read"}"#;
    let digest = crate::nats_pop::content_digest_sha256(body);
    let mut req = signed_body_request(&fixture, &jwt, body, digest);

    // Tamper with the covered content-digest header value itself.
    replace_header(
        &mut req,
        headers::CONTENT_DIGEST,
        crate::nats_pop::content_digest_sha256(b"different-payload"),
    );

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::BadSignature));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_tampered_covered_header() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(&fixture, &mut req, 1000, &REQUIRED_COMPONENTS);

    // Tamper with @path after signing.
    req.path = "/api/other-documents".to_string();

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::BadSignature));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_a_signature_made_for_another_authority() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    req.authority = "other-resource.example".to_string();
    sign_request(&fixture, &mut req, 1000, &REQUIRED_COMPONENTS);
    req.authority = "resource.example".to_string();

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::BadSignature));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_tampered_signature_bytes() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(&fixture, &mut req, 1000, &REQUIRED_COMPONENTS);

    let corrupted =
        "sig=:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA:";
    replace_header(&mut req, headers::SIGNATURE, corrupted.to_string());

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::BadSignature | HttpPopError::Verify(_)));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_accepts_a_signature_in_the_standard_base64_of_rfc_8941() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    let sig_input = signature_input_header(&REQUIRED_COMPONENTS, 1000);
    replace_header(&mut req, headers::SIGNATURE_INPUT, sig_input.clone());
    let parsed = SignatureInput::parse(&sig_input).expect("parse");
    let base = build_signature_base(&req, &parsed).expect("build base");
    let url_safe = sign(base.as_bytes(), &fixture.signing, fixture.alg).expect("sign");
    let bytes = URL_SAFE_NO_PAD.decode(url_safe).expect("decode");
    replace_header(
        &mut req,
        headers::SIGNATURE,
        format!("sig=:{}:", STANDARD.encode(bytes)),
    );

    verifier.verify(&req).await.expect("standard base64 signature verifies");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_created_outside_skew_window() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    // Signed at t=1000 but verified far outside the window.
    let verifier = verifier_at(jwks, 100_000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(&fixture, &mut req, 1000, &REQUIRED_COMPONENTS);

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::Freshness(FreshnessError::Expired)));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_tolerates_the_clock_skew_past_expiry_and_no_further() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");

    let within = verifier_at(
        jwks_with_key("agent-provider.example", fixture.jwk.clone()),
        1090,
        "resource.example",
    );
    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(&fixture, &mut req, 1000, &REQUIRED_COMPONENTS);
    within.verify(&req).await.expect("expired within the skew verifies");

    let beyond = verifier_at(
        jwks_with_key("agent-provider.example", fixture.jwk.clone()),
        1091,
        "resource.example",
    );
    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(&fixture, &mut req, 1000, &REQUIRED_COMPONENTS);
    let err = beyond.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::Freshness(FreshnessError::Expired)));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_tolerates_the_clock_skew_ahead_and_no_further() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(&fixture, &mut req, 1030, &REQUIRED_COMPONENTS);
    verifier.verify(&req).await.expect("created within the skew verifies");

    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(&fixture, &mut req, 1031, &REQUIRED_COMPONENTS);
    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::Freshness(FreshnessError::NotYetValid)));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_a_window_longer_than_the_accepted_validity() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    let sig_input = signature_input_with(&REQUIRED_COMPONENTS, 1000, 1061, &fresh_jti());
    sign_request_with_input(&fixture, &mut req, &sig_input);

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::Freshness(FreshnessError::InvalidWindow)));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_accepts_the_longest_window_when_configured_for_it() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let validity = SignatureValidity::new(crate::constants::MAX_SIGNATURE_VALIDITY_SECS).expect("validity");
    let verifier = verifier_at(jwks, 1000, "resource.example")
        .with_freshness(SignatureFreshness::new(validity, ClockTolerance::default()));

    let mut req = base_request(&signature_key_header(&jwt));
    let sig_input = signature_input_with(&REQUIRED_COMPONENTS, 1000, 1120, &fresh_jti());
    sign_request_with_input(&fixture, &mut req, &sig_input);

    verifier.verify(&req).await.expect("two-minute window verifies");
}

#[tokio::test(flavor = "current_thread")]
async fn new_bounds_the_presenting_token_expiry_by_the_clock_tolerance() {
    let fixture = p256_fixture("k1");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 5000, "resource.example");

    let within = agent_jwt_expiring(&fixture, "k1", "agent-provider.example", 4970);
    let mut req = base_request(&signature_key_header(&within));
    sign_request(&fixture, &mut req, 5000, &REQUIRED_COMPONENTS);
    verifier
        .verify(&req)
        .await
        .expect("token expired within the tolerance verifies");

    let beyond = agent_jwt_expiring(&fixture, "k1", "agent-provider.example", 4969);
    let mut req = base_request(&signature_key_header(&beyond));
    sign_request(&fixture, &mut req, 5000, &REQUIRED_COMPONENTS);
    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::Agent(TokenError::Expired)));
}

#[tokio::test(flavor = "current_thread")]
async fn with_token_leeway_narrows_the_presenting_token_expiry() {
    let fixture = p256_fixture("k1");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier =
        verifier_at(jwks, 5000, "resource.example").with_token_leeway(ClockTolerance::new(0).expect("tolerance"));

    let expired = agent_jwt_expiring(&fixture, "k1", "agent-provider.example", 4999);
    let mut req = base_request(&signature_key_header(&expired));
    sign_request(&fixture, &mut req, 5000, &REQUIRED_COMPONENTS);
    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::Agent(TokenError::Expired)));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_replayed_signature() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(&fixture, &mut req, 1000, &REQUIRED_COMPONENTS);

    verifier.verify(&req).await.expect("first request verifies");
    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::Replay));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_a_jti_reused_under_a_fresh_signature() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let mut first = base_request(&signature_key_header(&jwt));
    sign_request_with_input(
        &fixture,
        &mut first,
        &signature_input_with(&REQUIRED_COMPONENTS, 1000, 1060, "reused"),
    );
    verifier.verify(&first).await.expect("first use verifies");

    // Another request, signed afresh at a later time, still may not reuse it.
    let mut second = base_request(&signature_key_header(&jwt));
    second.path = "/api/other-documents".to_string();
    sign_request_with_input(
        &fixture,
        &mut second,
        &signature_input_with(&REQUIRED_COMPONENTS, 1001, 1061, "reused"),
    );
    let err = verifier.verify(&second).await.unwrap_err();
    assert!(matches!(err, HttpPopError::Replay));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_scopes_a_jti_to_the_key_that_signed_it() {
    let first_key = p256_fixture("k1");
    let second_key = p256_fixture("k2");
    let jwks = StaticJwks::new().with(
        "agent-provider.example",
        JwkSet {
            keys: vec![first_key.jwk.clone(), second_key.jwk.clone()],
        },
    );
    let verifier = verifier_at(jwks, 1000, "resource.example");
    let sig_input = signature_input_with(&REQUIRED_COMPONENTS, 1000, 1060, "shared");

    let mut first = base_request(&signature_key_header(&agent_jwt(
        &first_key,
        "k1",
        "agent-provider.example",
    )));
    sign_request_with_input(&first_key, &mut first, &sig_input);
    verifier.verify(&first).await.expect("first key verifies");

    let mut second = base_request(&signature_key_header(&agent_jwt(
        &second_key,
        "k2",
        "agent-provider.example",
    )));
    sign_request_with_input(&second_key, &mut second, &sig_input);
    verifier
        .verify(&second)
        .await
        .expect("the same jti from another key verifies");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_records_the_jti_under_the_signing_key_for_the_admissible_window() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let store = Arc::new(RecordingReplayStore::default());
    let verifier = verifier_with(jwks, 1000, Arc::clone(&store));

    let jti = "a b*c>";
    let mut req = base_request(&signature_key_header(&jwt));
    sign_request_with_input(
        &fixture,
        &mut req,
        &signature_input_with(&REQUIRED_COMPONENTS, 1000, 1060, jti),
    );
    let VerifiedPresenter::Agent(agent) = verifier.verify(&req).await.expect("verifies") else {
        panic!("expected agent presenter");
    };

    let records = store.records.lock().expect("records lock");
    let expected = format!(
        "jti.{}.{}",
        agent.jkt,
        URL_SAFE_NO_PAD.encode(Sha256::digest(jti.as_bytes()))
    );
    assert_eq!(*records, vec![(expected, 120)]);
    assert!(
        records[0]
            .0
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn verify_fails_closed_when_the_replay_store_errors() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_with(jwks, 1000, FailingReplayStore);

    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(&fixture, &mut req, 1000, &REQUIRED_COMPONENTS);

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::Backend(ReplayError::MutexPoisoned)));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_does_not_record_a_jti_for_a_request_that_fails_verification() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let store = Arc::new(RecordingReplayStore::default());
    let verifier = verifier_with(jwks, 1000, Arc::clone(&store));

    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(&fixture, &mut req, 1000, &REQUIRED_COMPONENTS);
    req.path = "/api/other-documents".to_string();

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::BadSignature));
    assert!(store.records.lock().expect("records lock").is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn verify_accepts_an_alg_naming_the_key_algorithm() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    let sig_input = format!(
        "{};alg=\"ecdsa-p256-sha256\"",
        signature_input_header(&REQUIRED_COMPONENTS, 1000)
    );
    sign_request_with_input(&fixture, &mut req, &sig_input);

    verifier.verify(&req).await.expect("matching alg verifies");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_an_alg_naming_another_algorithm() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    let sig_input = format!("{};alg=\"ed25519\"", signature_input_header(&REQUIRED_COMPONENTS, 1000));
    sign_request_with_input(&fixture, &mut req, &sig_input);

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::AlgorithmMismatch));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_a_signature_input_without_a_jti() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(&fixture, &mut req, 1000, &REQUIRED_COMPONENTS);
    replace_header(
        &mut req,
        headers::SIGNATURE_INPUT,
        r#"sig=("@method" "@authority" "@path" "signature-key" "content-digest");created=1000;expires=1060"#
            .to_string(),
    );

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(
        err,
        HttpPopError::MissingSignatureParameter(SignatureParameter::Nonce)
    ));
}

#[test]
fn build_signature_base_follows_rfc_9421_layout() {
    let req = base_request("sig=jwt;jwt=\"x\"");
    let input = SignatureInput::parse(
        r#"sig=( "@method" "@authority" "@path" "signature-key" "content-digest" );  created=1000;expires=1060;nonce="j""#,
    )
    .expect("parse");
    let base = build_signature_base(&req, &input).expect("build base");
    let expected = format!(
        concat!(
            "\"@method\": GET\n",
            "\"@authority\": resource.example\n",
            "\"@path\": /api/documents\n",
            "\"signature-key\": sig=jwt;jwt=\"x\"\n",
            "\"content-digest\": {digest}\n",
            "\"@signature-params\": (\"@method\" \"@authority\" \"@path\" \"signature-key\" \"content-digest\");created=1000;expires=1060;nonce=\"j\"",
        ),
        digest = empty_body_digest()
    );
    assert_eq!(base, expected);
}

#[test]
fn parse_signature_key_jwt_accepts_draft_shape() {
    let header = r#"sig=jwt;jwt="eyJhbGc...""#;
    let jwt = parse_signature_key_jwt(header).expect("parses");
    assert_eq!(jwt, "eyJhbGc...");
}

#[test]
fn parse_signature_key_jwt_rejects_unsupported_scheme() {
    let header = "sig=jwks_uri;";
    let err = parse_signature_key_jwt(header).unwrap_err();
    assert!(matches!(err, HttpPopError::UnsupportedSignatureKeyScheme));
}

#[test]
fn parse_signature_bytes_extracts_inner_base64() {
    let sig = parse_signature_bytes("sig=:AAEC:").expect("parses");
    assert_eq!(sig, "AAEC");
}

#[test]
fn parse_signature_bytes_reads_the_standard_alphabet() {
    let sig = parse_signature_bytes("sig=:+/8=:").expect("parses");
    assert_eq!(sig, "-_8");
}

#[test]
fn parse_signature_bytes_rejects_undecodable_bytes() {
    let err = parse_signature_bytes("sig=:not base64!:").unwrap_err();
    assert!(matches!(err, HttpPopError::MalformedSignature));
}

#[test]
fn verify_covered_components_rejects_missing_required_component() {
    let req = base_request("sig=jwt;jwt=\"x\"");
    let err = verify_covered_components(&req, &["@method".to_string(), "@authority".to_string()]).unwrap_err();
    assert!(matches!(err, HttpPopError::MissingCoveredComponent("@path")));
}

#[test]
fn verify_covered_components_requires_content_digest_coverage() {
    let req = base_request("sig=jwt;jwt=\"x\"");
    let components: Vec<String> = ["@method", "@authority", "@path", "signature-key"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let err = verify_covered_components(&req, &components).unwrap_err();
    assert!(matches!(err, HttpPopError::MissingContentDigest));
}

#[test]
fn verify_covered_components_requires_aauth_mission_when_header_present() {
    let mut req = base_request("sig=jwt;jwt=\"x\"");
    req.headers
        .push((headers::MISSION.to_string(), "approver=\"x\"; s256=\"y\"".to_string()));
    let components: Vec<String> = REQUIRED_COMPONENTS.iter().map(|s| s.to_string()).collect();
    let err = verify_covered_components(&req, &components).unwrap_err();
    assert!(matches!(err, HttpPopError::MissingMissionHeader));
}

#[test]
fn http_pop_verifier_new_wires_shared_clock_into_token_verifier() {
    let jwks = StaticJwks::new();
    let clock = FixedClock(42);
    let verifier = HttpPopVerifier::new(jwks, clock, InMemoryReplayStore::default(), "resource.example");
    assert_eq!(verifier.clock.now(), 42);
    assert_eq!(verifier.freshness, SignatureFreshness::default());
    assert_eq!(verifier.resource_identifier, "resource.example");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_duplicate_security_header() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(&fixture, &mut req, 1000, &REQUIRED_COMPONENTS);
    let dup = req.header(headers::SIGNATURE_KEY).unwrap().to_string();
    req.headers.push((headers::SIGNATURE_KEY.to_string(), dup));

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::DuplicateHeader(headers::SIGNATURE_KEY)));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_auth_presenter_with_structurally_incomplete_cnf_jwk() {
    let fixture = p256_fixture("k1");
    let mut header = jsonwebtoken::Header::new(fixture.alg);
    header.typ = Some(TYP_AUTH.into());
    header.kid = Some("k1".into());
    let claims = serde_json::json!({
        "iss": "as.example",
        "sub": "person-1",
        "aud": "resource.example",
        "jti": "j1",
        "iat": 1000,
        "exp": 9_999_999_999_i64,
        "agent": "aauth:asst@agent.example",
        "agent_jkt": "abc",
        "scope": "read",
        "cnf": {"jwk": {"kty": "EC"}},
    });
    let jwt = jsonwebtoken::encode(&header, &claims, &fixture.signing).expect("encode auth jwt");
    let jwks = jwks_with_key("as.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(&fixture, &mut req, 1000, &REQUIRED_COMPONENTS);

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(
        err,
        HttpPopError::InvalidConfirmationKey(InvalidConfirmationKeyError::StructurallyIncomplete(_))
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_unsupported_presenter_typ() {
    let fixture = p256_fixture("k1");
    let mut header = jsonwebtoken::Header::new(fixture.alg);
    header.typ = Some("some+other+jwt".into());
    header.kid = Some("k1".into());
    let claims = serde_json::json!({
        "iss": "agent-provider.example",
        "sub": "aauth:asst@agent.example",
        "jti": "j1",
        "iat": 1000,
        "exp": 9_999_999_999_i64,
        "dwk": "aauth-agent.json",
        "cnf": {"jwk": fixture.jwk_json},
    });
    let jwt = jsonwebtoken::encode(&header, &claims, &fixture.signing).expect("encode");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(&fixture, &mut req, 1000, &REQUIRED_COMPONENTS);

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::UnsupportedPresenterTyp(typ) if typ == "some+other+jwt"));
}

#[test]
fn presenter_kind_rejects_missing_typ() {
    let fixture = p256_fixture("k1");
    let mut header = jsonwebtoken::Header::new(fixture.alg);
    header.typ = None;
    let claims = serde_json::json!({"iss": "x"});
    let jwt = jsonwebtoken::encode(&header, &claims, &fixture.signing).expect("encode");

    let err = presenter_kind(&jwt).unwrap_err();
    assert!(matches!(err, HttpPopError::UnsupportedPresenterTyp(typ) if typ == "<none>"));
}

#[test]
fn presenter_kind_rejects_malformed_jwt() {
    let err = presenter_kind("not-a-jwt").unwrap_err();
    assert!(matches!(err, HttpPopError::UnsupportedPresenterTyp(typ) if typ.is_empty()));
}

#[test]
fn parse_signature_key_jwt_rejects_empty_token() {
    let header = r#"sig=jwt;jwt="""#;
    let err = parse_signature_key_jwt(header).unwrap_err();
    assert!(matches!(err, HttpPopError::UnsupportedSignatureKeyScheme));
}

#[test]
fn parse_signature_key_jwt_rejects_missing_jwt_param() {
    let header = "sig=jwt;other=1";
    let err = parse_signature_key_jwt(header).unwrap_err();
    assert!(matches!(err, HttpPopError::UnsupportedSignatureKeyScheme));
}

#[test]
fn parse_signature_bytes_rejects_empty_inner() {
    let header = "sig=::";
    let err = parse_signature_bytes(header).unwrap_err();
    assert!(matches!(err, HttpPopError::MalformedSignature));
}

#[test]
fn parse_signature_bytes_rejects_missing_colon_wrapper() {
    let header = "sig=BASE64";
    let err = parse_signature_bytes(header).unwrap_err();
    assert!(matches!(err, HttpPopError::MalformedSignature));
}

#[test]
fn extract_quoted_rejects_token_with_interior_quote() {
    assert_eq!(extract_quoted("ab\"cd"), None);
}

#[test]
fn extract_quoted_accepts_bare_token() {
    assert_eq!(extract_quoted("jwt"), Some("jwt".to_string()));
}

#[test]
fn verify_covered_components_rejects_missing_signature_key_component() {
    let req = base_request("sig=jwt;jwt=\"x\"");
    let err = verify_covered_components(
        &req,
        &["@method".to_string(), "@authority".to_string(), "@path".to_string()],
    )
    .unwrap_err();
    assert!(matches!(err, HttpPopError::MissingCoveredComponent("signature-key")));
}

#[test]
fn verify_covered_components_rejects_when_content_digest_header_absent_despite_coverage() {
    let mut req = base_request("sig=jwt;jwt=\"x\"");
    remove_header(&mut req, headers::CONTENT_DIGEST);
    let components: Vec<String> = REQUIRED_COMPONENTS.iter().map(|s| s.to_string()).collect();
    let err = verify_covered_components(&req, &components).unwrap_err();
    assert!(matches!(err, HttpPopError::MissingContentDigest));
}

#[test]
fn verify_covered_components_rejects_when_mission_component_covered_but_header_absent() {
    let req = base_request("sig=jwt;jwt=\"x\"");
    let mut components: Vec<String> = REQUIRED_COMPONENTS.iter().map(|s| s.to_string()).collect();
    components.push("aauth-mission".to_string());
    let err = verify_covered_components(&req, &components).unwrap_err();
    assert!(matches!(err, HttpPopError::MissingMissionHeader));
}

#[test]
fn verify_covered_components_accepts_matching_mission_coverage() {
    let mut req = base_request("sig=jwt;jwt=\"x\"");
    req.headers
        .push((headers::MISSION.to_string(), "approver=\"x\"; s256=\"y\"".to_string()));
    let mut components: Vec<String> = REQUIRED_COMPONENTS.iter().map(|s| s.to_string()).collect();
    components.push("aauth-mission".to_string());
    verify_covered_components(&req, &components).expect("mission coverage matches header presence");
}

#[test]
fn component_value_rejects_signature_key_header_absent() {
    let req = HttpRequest {
        method: "GET".to_string(),
        authority: "resource.example".to_string(),
        path: "/api/documents".to_string(),
        headers: vec![],
        body: None,
    };
    let err = component_value(&req, "signature-key").unwrap_err();
    assert!(matches!(err, HttpPopError::MissingHeader(headers::SIGNATURE_KEY)));
}

#[test]
fn component_value_rejects_content_digest_header_absent() {
    let mut req = base_request("sig=jwt;jwt=\"x\"");
    remove_header(&mut req, headers::CONTENT_DIGEST);
    let err = component_value(&req, "content-digest").unwrap_err();
    assert!(matches!(err, HttpPopError::MissingContentDigest));
}

#[test]
fn component_value_rejects_aauth_mission_header_absent() {
    let req = base_request("sig=jwt;jwt=\"x\"");
    let err = component_value(&req, "aauth-mission").unwrap_err();
    assert!(matches!(err, HttpPopError::MissingMissionHeader));
}

#[test]
fn verify_covered_components_requires_aauth_nonce_coverage_when_header_present() {
    let mut req = base_request("sig=jwt;jwt=\"x\"");
    req.headers.push((SERVER_NONCE_HEADER.to_string(), "n".to_string()));
    let components: Vec<String> = REQUIRED_COMPONENTS.iter().map(|s| s.to_string()).collect();
    let err = verify_covered_components(&req, &components).unwrap_err();
    assert!(matches!(err, HttpPopError::UnboundNonce));
}

#[test]
fn verify_covered_components_rejects_a_covered_aauth_nonce_that_is_absent() {
    let req = base_request("sig=jwt;jwt=\"x\"");
    let components: Vec<String> = NONCE_COMPONENTS.iter().map(|s| s.to_string()).collect();
    let err = verify_covered_components(&req, &components).unwrap_err();
    assert!(matches!(err, HttpPopError::UnboundNonce));
}

#[test]
fn component_value_rejects_aauth_nonce_header_absent() {
    let req = base_request("sig=jwt;jwt=\"x\"");
    let err = component_value(&req, SERVER_NONCE_COMPONENT).unwrap_err();
    assert!(matches!(err, HttpPopError::UnboundNonce));
}

#[test]
fn component_value_rejects_unsupported_component_name() {
    let req = base_request("sig=jwt;jwt=\"x\"");
    let err = component_value(&req, "x-custom-header").unwrap_err();
    assert!(matches!(
        err,
        HttpPopError::MissingCoveredComponent("unsupported component")
    ));
}

#[test]
fn component_value_accepts_arbitrary_covered_header() {
    let mut req = base_request("sig=jwt;jwt=\"x\"");
    req.headers
        .push(("x-custom-header".to_string(), "  a   b  ".to_string()));
    let value = component_value(&req, "x-custom-header").expect("covered header resolves");
    assert_eq!(value, "a b");
}

#[test]
fn verify_signature_with_jwk_rejects_undeserializable_jwk() {
    let jwk_val = serde_json::json!({"kty": "unsupported"});
    let err = verify_signature_with_jwk(&jwk_val, None, b"base", "sig").unwrap_err();
    assert!(matches!(
        err,
        HttpPopError::InvalidConfirmationKey(InvalidConfirmationKeyError::Deserialize(_))
    ));
}

#[test]
fn verify_signature_with_jwk_rejects_unsupported_curve() {
    let jwk_val = serde_json::json!({
        "kty": "EC",
        "crv": "P-521",
        "x": "AAAA",
        "y": "AAAA",
    });
    let err = verify_signature_with_jwk(&jwk_val, None, b"base", "sig").unwrap_err();
    assert!(matches!(
        err,
        HttpPopError::InvalidConfirmationKey(InvalidConfirmationKeyError::UnsupportedAlgorithm)
    ));
}

#[test]
fn verify_signature_with_jwk_accepts_p384_key_and_verifies_signature() {
    let fixture = p384_fixture();
    let base = b"some signature base";
    let sig_b64 = jsonwebtoken::crypto::sign(base, &fixture.signing, fixture.alg).expect("sign");
    verify_signature_with_jwk(&fixture.jwk_json, None, base, &sig_b64).expect("p384 signature verifies");
    verify_signature_with_jwk(&fixture.jwk_json, Some("ecdsa-p384-sha384"), base, &sig_b64)
        .expect("p384 signature verifies under its alg");
}

#[test]
fn verify_signature_with_jwk_rejects_p384_signature_over_wrong_base() {
    let fixture = p384_fixture();
    let sig_b64 = jsonwebtoken::crypto::sign(b"expected base", &fixture.signing, fixture.alg).expect("sign");
    let err = verify_signature_with_jwk(&fixture.jwk_json, None, b"different base", &sig_b64).unwrap_err();
    assert!(matches!(err, HttpPopError::BadSignature));
}

#[test]
fn verify_signature_with_jwk_rejects_a_p384_key_declared_as_p256() {
    let fixture = p384_fixture();
    let base = b"some signature base";
    let sig_b64 = jsonwebtoken::crypto::sign(base, &fixture.signing, fixture.alg).expect("sign");
    let err = verify_signature_with_jwk(&fixture.jwk_json, Some("ecdsa-p256-sha256"), base, &sig_b64).unwrap_err();
    assert!(matches!(err, HttpPopError::AlgorithmMismatch));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_accepts_es384_agent_presenter() {
    let fixture = p384_fixture();
    let jwt = agent_jwt(&fixture, "p384-k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(&fixture, &mut req, 1000, &REQUIRED_COMPONENTS);

    let result = verifier.verify(&req).await.expect("es384 agent presenter verifies");
    assert!(matches!(result, VerifiedPresenter::Agent(_)));
}

#[test]
fn http_pop_error_display_messages_are_distinct() {
    let issued = server_nonces(&challenge_key_pem(), NonceRequirement::Required)
        .mint("resource.example", "jkt", 1000)
        .expect("mint");
    let cases = [
        format!("{}", HttpPopError::MissingHeader("x")),
        format!("{}", HttpPopError::DuplicateHeader("x")),
        format!("{}", HttpPopError::UnsupportedSignatureKeyScheme),
        format!("{}", HttpPopError::MalformedSignatureInput),
        format!(
            "{}",
            HttpPopError::MissingSignatureParameter(SignatureParameter::Created)
        ),
        format!(
            "{}",
            HttpPopError::InvalidSignatureParameter(SignatureParameter::Created)
        ),
        format!("{}", HttpPopError::InvalidJti(InvalidJtiError::Empty)),
        format!("{}", HttpPopError::MalformedSignature),
        format!("{}", HttpPopError::MissingContentDigest),
        format!("{}", HttpPopError::Freshness(FreshnessError::Expired)),
        format!("{}", HttpPopError::Freshness(FreshnessError::NotYetValid)),
        format!("{}", HttpPopError::AlgorithmMismatch),
        format!("{}", HttpPopError::Replay),
        format!("{}", HttpPopError::BadSignature),
        format!("{}", HttpPopError::UnboundNonce),
        format!(
            "{}",
            HttpPopError::NonceRequired(NonceChallenge::new(issued.nonce, None))
        ),
        format!("{}", HttpPopError::InvalidNonce(NonceError::NotIssued)),
        format!("{}", HttpPopError::NonceIssue(NonceIssueError::Collision)),
    ];
    for i in 0..cases.len() {
        for j in (i + 1)..cases.len() {
            assert_ne!(cases[i], cases[j]);
        }
    }
}

const NONCE_COMPONENTS: [&str; 6] = [
    "@method",
    "@authority",
    "@path",
    "signature-key",
    "content-digest",
    SERVER_NONCE_COMPONENT,
];

fn server_nonces(pem: &str, requirement: NonceRequirement) -> ServerNonces {
    ServerNonces::new(challenge_key_from(pem, "nonce-k1"), NonceTtl::default(), requirement)
}

fn nonce_verifier<S: ReplayStore>(
    jwks: StaticJwks,
    now: i64,
    replay: S,
    pem: &str,
    requirement: NonceRequirement,
) -> HttpPopVerifier<StaticJwks, FixedClock, S> {
    verifier_with(jwks, now, replay).with_server_nonces(server_nonces(pem, requirement))
}

/// An agent signing key, its presenting token and the JWKS that verifies it.
fn agent(kid: &str) -> (EcFixture, String, StaticJwks) {
    let fixture = p256_fixture(kid);
    let jwt = agent_jwt(&fixture, kid, "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    (fixture, jwt, jwks)
}

/// A request signed at `created` under a fresh `jti`, binding `nonce` when
/// one is given.
fn nonce_request(fixture: &EcFixture, jwt: &str, created: i64, nonce: Option<&ServerNonce>) -> HttpRequest {
    let mut req = base_request(&signature_key_header(jwt));
    match nonce {
        Some(nonce) => {
            req.headers
                .push((SERVER_NONCE_HEADER.to_string(), nonce.as_str().to_string()));
            sign_request(fixture, &mut req, created, &NONCE_COMPONENTS);
        }
        None => sign_request(fixture, &mut req, created, &REQUIRED_COMPONENTS),
    }
    req
}

fn expect_challenge(err: HttpPopError) -> NonceChallenge {
    let HttpPopError::NonceRequired(challenge) = err else {
        panic!("expected a nonce challenge, got {err:?}");
    };
    challenge
}

/// The challenge a request binding no nonce draws from `verifier`.
async fn challenge<S: ReplayStore>(
    verifier: &HttpPopVerifier<StaticJwks, FixedClock, S>,
    fixture: &EcFixture,
    jwt: &str,
    created: i64,
) -> NonceChallenge {
    let err = verifier
        .verify(&nonce_request(fixture, jwt, created, None))
        .await
        .unwrap_err();
    expect_challenge(err)
}

#[tokio::test(flavor = "current_thread")]
async fn verify_demands_a_nonce_and_admits_the_retry_that_binds_it() {
    let (fixture, jwt, jwks) = agent("k1");
    let verifier = nonce_verifier(
        jwks,
        1000,
        InMemoryReplayStore::default(),
        &challenge_key_pem(),
        NonceRequirement::Required,
    );

    let challenge = challenge(&verifier, &fixture, &jwt, 1000).await;
    assert!(challenge.refused().is_none());

    let retry = nonce_request(&fixture, &jwt, 1000, Some(challenge.nonce()));
    let result = verifier.verify(&retry).await.expect("retry binding the nonce verifies");
    assert!(matches!(result, VerifiedPresenter::Agent(_)));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_admits_a_nonce_on_every_request_while_it_lives() {
    let (fixture, jwt, jwks) = agent("k1");
    let verifier = nonce_verifier(
        jwks,
        1000,
        InMemoryReplayStore::default(),
        &challenge_key_pem(),
        NonceRequirement::Required,
    );
    let challenge = challenge(&verifier, &fixture, &jwt, 1000).await;

    for _ in 0..2 {
        let retry = nonce_request(&fixture, &jwt, 1000, Some(challenge.nonce()));
        verifier
            .verify(&retry)
            .await
            .expect("a request under a fresh jti verifies");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_an_aauth_nonce_left_out_of_coverage() {
    let (fixture, jwt, jwks) = agent("k1");
    let verifier = nonce_verifier(
        jwks,
        1000,
        InMemoryReplayStore::default(),
        &challenge_key_pem(),
        NonceRequirement::Required,
    );
    let challenge = challenge(&verifier, &fixture, &jwt, 1000).await;

    let mut req = base_request(&signature_key_header(&jwt));
    req.headers
        .push((SERVER_NONCE_HEADER.to_string(), challenge.nonce().as_str().to_string()));
    sign_request(&fixture, &mut req, 1000, &REQUIRED_COMPONENTS);

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::UnboundNonce), "got {err:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_a_covered_aauth_nonce_that_was_stripped() {
    let (fixture, jwt, jwks) = agent("k1");
    let verifier = nonce_verifier(
        jwks,
        1000,
        InMemoryReplayStore::default(),
        &challenge_key_pem(),
        NonceRequirement::Required,
    );
    let challenge = challenge(&verifier, &fixture, &jwt, 1000).await;

    let mut req = nonce_request(&fixture, &jwt, 1000, Some(challenge.nonce()));
    remove_header(&mut req, SERVER_NONCE_HEADER);

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::UnboundNonce), "got {err:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_a_duplicated_aauth_nonce() {
    let (fixture, jwt, jwks) = agent("k1");
    let verifier = nonce_verifier(
        jwks,
        1000,
        InMemoryReplayStore::default(),
        &challenge_key_pem(),
        NonceRequirement::Required,
    );
    let challenge = challenge(&verifier, &fixture, &jwt, 1000).await;

    let mut req = nonce_request(&fixture, &jwt, 1000, Some(challenge.nonce()));
    req.headers
        .push((SERVER_NONCE_HEADER.to_string(), challenge.nonce().as_str().to_string()));

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(
        matches!(err, HttpPopError::DuplicateHeader(SERVER_NONCE_HEADER)),
        "got {err:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rechallenges_a_nonce_issued_to_another_key() {
    let first = p256_fixture("k1");
    let second = p256_fixture("k2");
    let jwks = StaticJwks::new().with(
        "agent-provider.example",
        JwkSet {
            keys: vec![first.jwk.clone(), second.jwk.clone()],
        },
    );
    let first_jwt = agent_jwt(&first, "k1", "agent-provider.example");
    let second_jwt = agent_jwt(&second, "k2", "agent-provider.example");
    let verifier = nonce_verifier(
        jwks,
        1000,
        InMemoryReplayStore::default(),
        &challenge_key_pem(),
        NonceRequirement::Required,
    );
    let issued_to_first = challenge(&verifier, &first, &first_jwt, 1000).await;

    let borrowed = nonce_request(&second, &second_jwt, 1000, Some(issued_to_first.nonce()));
    let rechallenge = expect_challenge(verifier.verify(&borrowed).await.unwrap_err());

    assert!(
        matches!(rechallenge.refused(), Some(NonceError::KeyMismatch)),
        "got {:?}",
        rechallenge.refused()
    );
    let retry = nonce_request(&second, &second_jwt, 1000, Some(rechallenge.nonce()));
    verifier
        .verify(&retry)
        .await
        .expect("the nonce issued to the second key verifies");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rechallenges_a_nonce_another_challenge_key_signed() {
    let (fixture, jwt, jwks) = agent("k1");
    let verifier = nonce_verifier(
        jwks.clone(),
        1000,
        InMemoryReplayStore::default(),
        &challenge_key_pem(),
        NonceRequirement::Required,
    );
    let impostor = nonce_verifier(
        jwks,
        1000,
        InMemoryReplayStore::default(),
        &challenge_key_pem(),
        NonceRequirement::Required,
    );
    let forged = challenge(&impostor, &fixture, &jwt, 1000).await;

    let req = nonce_request(&fixture, &jwt, 1000, Some(forged.nonce()));
    let rechallenge = expect_challenge(verifier.verify(&req).await.unwrap_err());

    assert!(
        matches!(rechallenge.refused(), Some(NonceError::Rejected(_))),
        "got {:?}",
        rechallenge.refused()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rechallenges_an_expired_nonce() {
    let (fixture, jwt, jwks) = agent("k1");
    let pem = challenge_key_pem();
    let store = Arc::new(InMemoryReplayStore::default());
    let at_issue = nonce_verifier(jwks.clone(), 1000, Arc::clone(&store), &pem, NonceRequirement::Required);
    let past_expiry = nonce_verifier(jwks, 1091, Arc::clone(&store), &pem, NonceRequirement::Required);
    let stale = challenge(&at_issue, &fixture, &jwt, 1000).await;

    let req = nonce_request(&fixture, &jwt, 1091, Some(stale.nonce()));
    let rechallenge = expect_challenge(past_expiry.verify(&req).await.unwrap_err());

    assert!(
        matches!(rechallenge.refused(), Some(NonceError::Expired)),
        "got {:?}",
        rechallenge.refused()
    );
    assert_ne!(rechallenge.nonce(), stale.nonce());
    let retry = nonce_request(&fixture, &jwt, 1091, Some(rechallenge.nonce()));
    past_expiry.verify(&retry).await.expect("the fresh nonce verifies");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_honors_a_nonce_another_instance_issued() {
    let (fixture, jwt, jwks) = agent("k1");
    let pem = challenge_key_pem();
    let store = Arc::new(InMemoryReplayStore::default());
    let issuing = nonce_verifier(jwks.clone(), 1000, Arc::clone(&store), &pem, NonceRequirement::Required);
    let other = nonce_verifier(jwks, 1000, Arc::clone(&store), &pem, NonceRequirement::Required);
    let challenge = challenge(&issuing, &fixture, &jwt, 1000).await;

    let retry = nonce_request(&fixture, &jwt, 1000, Some(challenge.nonce()));
    other
        .verify(&retry)
        .await
        .expect("an instance sharing the store and key honors the nonce");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_refuses_a_nonce_no_shared_record_shows_was_issued() {
    let (fixture, jwt, jwks) = agent("k1");
    let pem = challenge_key_pem();
    let issuing = nonce_verifier(
        jwks.clone(),
        1000,
        InMemoryReplayStore::default(),
        &pem,
        NonceRequirement::Required,
    );
    let optional = nonce_verifier(
        jwks.clone(),
        1000,
        InMemoryReplayStore::default(),
        &pem,
        NonceRequirement::Optional,
    );
    let required = nonce_verifier(
        jwks,
        1000,
        InMemoryReplayStore::default(),
        &pem,
        NonceRequirement::Required,
    );
    let challenge = challenge(&issuing, &fixture, &jwt, 1000).await;

    let req = nonce_request(&fixture, &jwt, 1000, Some(challenge.nonce()));
    let err = optional.verify(&req).await.unwrap_err();
    assert!(
        matches!(err, HttpPopError::InvalidNonce(NonceError::NotIssued)),
        "got {err:?}"
    );

    let rechallenge = expect_challenge(required.verify(&req).await.unwrap_err());
    assert!(
        matches!(rechallenge.refused(), Some(NonceError::NotIssued)),
        "got {:?}",
        rechallenge.refused()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn verify_admits_a_request_binding_no_nonce_when_nonces_are_optional() {
    let (fixture, jwt, jwks) = agent("k1");
    let verifier = nonce_verifier(
        jwks,
        1000,
        InMemoryReplayStore::default(),
        &challenge_key_pem(),
        NonceRequirement::Optional,
    );

    let req = nonce_request(&fixture, &jwt, 1000, None);
    verifier.verify(&req).await.expect("an optional nonce may be left out");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_refuses_an_unusable_nonce_when_nonces_are_optional() {
    let (fixture, jwt, jwks) = agent("k1");
    let verifier = nonce_verifier(
        jwks,
        1000,
        InMemoryReplayStore::default(),
        &challenge_key_pem(),
        NonceRequirement::Optional,
    );
    let mut req = base_request(&signature_key_header(&jwt));
    req.headers
        .push((SERVER_NONCE_HEADER.to_string(), "not-a-nonce".to_string()));
    sign_request(&fixture, &mut req, 1000, &NONCE_COMPONENTS);

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(
        matches!(err, HttpPopError::InvalidNonce(NonceError::Malformed(_))),
        "got {err:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn verify_refuses_a_bound_nonce_when_the_resource_issues_none() {
    let (fixture, jwt, jwks) = agent("k1");
    let store = Arc::new(InMemoryReplayStore::default());
    let issuing = nonce_verifier(
        jwks.clone(),
        1000,
        Arc::clone(&store),
        &challenge_key_pem(),
        NonceRequirement::Required,
    );
    let without_nonces = verifier_with(jwks, 1000, Arc::clone(&store));
    let challenge = challenge(&issuing, &fixture, &jwt, 1000).await;

    let req = nonce_request(&fixture, &jwt, 1000, Some(challenge.nonce()));
    let err = without_nonces.verify(&req).await.unwrap_err();
    assert!(
        matches!(err, HttpPopError::InvalidNonce(NonceError::NotIssued)),
        "got {err:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn verify_fails_closed_when_the_nonce_record_cannot_be_looked_up() {
    let (fixture, jwt, jwks) = agent("k1");
    let pem = challenge_key_pem();
    let issuing = nonce_verifier(
        jwks.clone(),
        1000,
        InMemoryReplayStore::default(),
        &pem,
        NonceRequirement::Required,
    );
    let blind = nonce_verifier(
        jwks,
        1000,
        LookupFailingReplayStore::default(),
        &pem,
        NonceRequirement::Required,
    );
    let challenge = challenge(&issuing, &fixture, &jwt, 1000).await;

    let req = nonce_request(&fixture, &jwt, 1000, Some(challenge.nonce()));
    let err = blind.verify(&req).await.unwrap_err();
    assert!(
        matches!(err, HttpPopError::Backend(ReplayError::MutexPoisoned)),
        "got {err:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn verify_refuses_a_replayed_request_before_demanding_a_nonce() {
    let (fixture, jwt, jwks) = agent("k1");
    let verifier = nonce_verifier(
        jwks,
        1000,
        InMemoryReplayStore::default(),
        &challenge_key_pem(),
        NonceRequirement::Required,
    );
    let req = nonce_request(&fixture, &jwt, 1000, None);

    expect_challenge(verifier.verify(&req).await.unwrap_err());
    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::Replay), "got {err:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_records_an_issued_nonce_for_its_lifetime_plus_the_skew_on_either_side() {
    let (fixture, jwt, jwks) = agent("k1");
    let store = Arc::new(RecordingReplayStore::default());
    let verifier = nonce_verifier(
        jwks,
        1000,
        Arc::clone(&store),
        &challenge_key_pem(),
        NonceRequirement::Required,
    );

    challenge(&verifier, &fixture, &jwt, 1000).await;

    let records = store.records.lock().expect("records lock");
    assert_eq!(records.len(), 2);
    assert!(records[0].0.starts_with("jti."));
    assert!(records[1].0.starts_with("nonce."));
    assert_eq!(records[1].1, 120);
}

#[tokio::test(flavor = "current_thread")]
async fn verify_reports_a_nonce_record_that_already_exists() {
    let (fixture, jwt, jwks) = agent("k1");
    let verifier = nonce_verifier(
        jwks,
        1000,
        CollidingNonceStore,
        &challenge_key_pem(),
        NonceRequirement::Required,
    );

    let err = verifier
        .verify(&nonce_request(&fixture, &jwt, 1000, None))
        .await
        .unwrap_err();
    assert!(
        matches!(err, HttpPopError::NonceIssue(NonceIssueError::Collision)),
        "got {err:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn verify_issues_no_nonce_to_a_request_that_fails_verification() {
    let (fixture, jwt, jwks) = agent("k1");
    let store = Arc::new(RecordingReplayStore::default());
    let verifier = nonce_verifier(
        jwks,
        1000,
        Arc::clone(&store),
        &challenge_key_pem(),
        NonceRequirement::Required,
    );
    let mut req = nonce_request(&fixture, &jwt, 1000, None);
    req.path = "/api/other-documents".to_string();

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::BadSignature), "got {err:?}");
    assert!(store.records.lock().expect("records lock").is_empty());
}
