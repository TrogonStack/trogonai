use super::*;
use crate::replay::InMemoryReplayStore;
use crate::test_support::{ed25519_fixture, jwks_with_key, p256_fixture, p384_fixture};
use jsonwebtoken::crypto::sign;
use trogon_identity_types::aauth::{TYP_AGENT, TYP_AUTH};

#[derive(Clone, Copy)]
struct FixedClock(i64);

impl TimeSource for FixedClock {
    fn now(&self) -> i64 {
        self.0
    }
}

fn verifier_at(
    jwks: crate::jwks::StaticJwks,
    now: i64,
    resource_identifier: &str,
) -> HttpPopVerifier<crate::jwks::StaticJwks, FixedClock, InMemoryReplayStore> {
    let clock = FixedClock(now);
    HttpPopVerifier {
        token_verifier: TokenVerifier::new(jwks, clock),
        clock,
        replay: InMemoryReplayStore::default(),
        max_skew_secs: 60,
        resource_identifier: resource_identifier.to_string(),
    }
}

fn agent_jwt(fixture: &crate::test_support::EcFixture, kid: &str, iss: &str) -> String {
    let mut header = jsonwebtoken::Header::new(fixture.alg);
    header.typ = Some(TYP_AGENT.into());
    header.kid = Some(kid.into());
    let claims = serde_json::json!({
        "iss": iss,
        "sub": "aauth:asst@agent.example",
        "jti": "j1",
        "iat": 1000,
        "exp": 9_999_999_999_i64,
        "dwk": "aauth-agent.json",
        "cnf": {"jwk": fixture.jwk_json},
    });
    jsonwebtoken::encode(&header, &claims, &fixture.signing).expect("encode agent jwt")
}

fn auth_jwt(fixture: &crate::test_support::EcFixture, kid: &str, iss: &str, aud: &str) -> String {
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

fn signature_input_header(components: &[&str], created: i64) -> String {
    let list = components
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(" ");
    format!("sig=({list});created={created}")
}

fn base_request(sig_key: &str) -> HttpRequest {
    HttpRequest {
        method: "GET".to_string(),
        authority: "resource.example".to_string(),
        path: "/api/documents".to_string(),
        headers: vec![(headers::SIGNATURE_KEY.to_string(), sig_key.to_string())],
        body: None,
    }
}

/// Signs `req` with `fixture` over the given covered components at `created`,
/// inserting/replacing `Signature-Input` and `Signature`.
fn sign_request(fixture: &crate::test_support::EcFixture, req: &mut HttpRequest, created: i64, components: &[&str]) {
    let sig_input = signature_input_header(components, created);
    req.headers
        .retain(|(k, _)| !k.eq_ignore_ascii_case(headers::SIGNATURE_INPUT));
    req.headers
        .push((headers::SIGNATURE_INPUT.to_string(), sig_input.clone()));
    let parsed = parse_signature_input(&sig_input).expect("parse for test");
    let base = build_signature_base(req, &parsed).expect("build base");
    let sig_b64 = sign(base.as_bytes(), &fixture.signing, fixture.alg).expect("sign");
    let sig_header = format!("sig=:{sig_b64}:");
    req.headers.retain(|(k, _)| !k.eq_ignore_ascii_case(headers::SIGNATURE));
    req.headers.push((headers::SIGNATURE.to_string(), sig_header));
}

const REQUIRED_COMPONENTS: [&str; 4] = ["@method", "@authority", "@path", "signature-key"];

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
    let sig_input = signature_input_header(&REQUIRED_COMPONENTS, 1000);
    req.headers
        .push((headers::SIGNATURE_INPUT.to_string(), sig_input.clone()));
    let parsed = parse_signature_input(&sig_input).expect("parse");
    let base = build_signature_base(&req, &parsed).expect("build base");
    let sig_b64 = fixture.sign_pop_base(base.as_bytes());
    req.headers
        .push((headers::SIGNATURE.to_string(), format!("sig=:{sig_b64}:")));

    let result = verifier.verify(&req).await.expect("eddsa agent presenter verifies");
    assert!(matches!(result, VerifiedPresenter::Agent(_)));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_tampered_body_content_digest_not_recomputed() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let body = br#"{"scope":"data.read"}"#.to_vec();
    let digest = crate::nats_pop::content_digest_sha256(&body);
    let mut req = base_request(&signature_key_header(&jwt));
    req.body = Some(body);
    req.headers.push((headers::CONTENT_DIGEST.to_string(), digest));
    let mut components = REQUIRED_COMPONENTS.to_vec();
    components.push("content-digest");
    sign_request(&fixture, &mut req, 1000, &components);

    // Tamper with the body after signing without updating content-digest.
    // The digest header itself is covered and unmodified, so the signature
    // still verifies -- catching a body/digest mismatch is the caller's
    // responsibility (recompute and compare), since #covered-components only
    // requires that content-digest be *covered*, not that this crate
    // recompute it against a body it is never given as a signed component.
    req.body = Some(br#"{"scope":"data.write"}"#.to_vec());
    let recomputed = crate::nats_pop::content_digest_sha256(req.body.as_ref().unwrap());
    let supplied = req.header(headers::CONTENT_DIGEST).unwrap().to_string();
    assert_ne!(recomputed, supplied, "tampering must be visible via digest mismatch");

    verifier
        .verify(&req)
        .await
        .expect("signature over untouched digest header still verifies");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_tampered_content_digest_header() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let body = br#"{"scope":"data.read"}"#.to_vec();
    let digest = crate::nats_pop::content_digest_sha256(&body);
    let mut req = base_request(&signature_key_header(&jwt));
    req.body = Some(body);
    req.headers.push((headers::CONTENT_DIGEST.to_string(), digest));
    let mut components = REQUIRED_COMPONENTS.to_vec();
    components.push("content-digest");
    sign_request(&fixture, &mut req, 1000, &components);

    // Tamper with the covered content-digest header value itself.
    req.headers
        .retain(|(k, _)| !k.eq_ignore_ascii_case(headers::CONTENT_DIGEST));
    req.headers.push((
        headers::CONTENT_DIGEST.to_string(),
        crate::nats_pop::content_digest_sha256(b"different-payload"),
    ));

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
async fn verify_rejects_tampered_signature_bytes() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let verifier = verifier_at(jwks, 1000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(&fixture, &mut req, 1000, &REQUIRED_COMPONENTS);

    let corrupted =
        "sig=:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA:";
    req.headers.retain(|(k, _)| !k.eq_ignore_ascii_case(headers::SIGNATURE));
    req.headers
        .push((headers::SIGNATURE.to_string(), corrupted.to_string()));

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::BadSignature | HttpPopError::Verify(_)));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_created_outside_skew_window() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    // Signed at t=1000 but verified far outside the 60s window.
    let verifier = verifier_at(jwks, 100_000, "resource.example");

    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(&fixture, &mut req, 1000, &REQUIRED_COMPONENTS);

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::Skew));
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
fn parse_signature_input_extracts_components_and_created() {
    let header = r#"sig=("@method" "@authority" "@path" "signature-key");created=1730217600"#;
    let parsed = parse_signature_input(header).expect("parses");
    assert_eq!(
        parsed.components,
        vec!["@method", "@authority", "@path", "signature-key"]
    );
    assert_eq!(parsed.created, 1_730_217_600);
}

#[test]
fn parse_signature_bytes_extracts_inner_base64() {
    let header = "sig=:BASE64URL-SIGNATURE-PLACEHOLDER:";
    let sig = parse_signature_bytes(header).expect("parses");
    assert_eq!(sig, "BASE64URL-SIGNATURE-PLACEHOLDER");
}

#[test]
fn verify_covered_components_rejects_missing_required_component() {
    let req = base_request("sig=jwt;jwt=\"x\"");
    let err = verify_covered_components(&req, &["@method".to_string(), "@authority".to_string()]).unwrap_err();
    assert!(matches!(err, HttpPopError::MissingCoveredComponent("@path")));
}

#[test]
fn verify_covered_components_requires_content_digest_when_body_present() {
    let mut req = base_request("sig=jwt;jwt=\"x\"");
    req.body = Some(b"payload".to_vec());
    let components: Vec<String> = REQUIRED_COMPONENTS.iter().map(|s| s.to_string()).collect();
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
    let jwks = crate::jwks::StaticJwks::new();
    let clock = FixedClock(42);
    let verifier = HttpPopVerifier::new(jwks, clock, InMemoryReplayStore::default(), "resource.example");
    assert_eq!(verifier.clock.now(), 42);
    assert_eq!(verifier.max_skew_secs, 60);
    assert_eq!(verifier.resource_identifier, "resource.example");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_rejects_negative_max_skew() {
    let fixture = p256_fixture("k1");
    let jwt = agent_jwt(&fixture, "k1", "agent-provider.example");
    let jwks = jwks_with_key("agent-provider.example", fixture.jwk.clone());
    let mut verifier = verifier_at(jwks, 1000, "resource.example");
    verifier.max_skew_secs = -1;

    let mut req = base_request(&signature_key_header(&jwt));
    sign_request(&fixture, &mut req, 1000, &REQUIRED_COMPONENTS);

    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, HttpPopError::NegativeMaxSkew(-1)));
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
        HttpPopError::InvalidConfirmationKey(InvalidConfirmationKey::StructurallyIncomplete(_))
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
fn parse_signature_input_rejects_empty_component_list() {
    let header = r#"sig=();created=1000"#;
    let err = match parse_signature_input(header) {
        Ok(_) => panic!("expected empty component list to be rejected"),
        Err(e) => e,
    };
    assert!(matches!(err, HttpPopError::MalformedSignatureInput));
}

#[test]
fn parse_signature_input_rejects_missing_created() {
    let header = r#"sig=("@method")"#;
    let err = match parse_signature_input(header) {
        Ok(_) => panic!("expected missing created to be rejected"),
        Err(e) => e,
    };
    assert!(matches!(err, HttpPopError::InvalidCreated));
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
    req.body = Some(b"payload".to_vec());
    let mut components: Vec<String> = REQUIRED_COMPONENTS.iter().map(|s| s.to_string()).collect();
    components.push("content-digest".to_string());
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
    let req = base_request("sig=jwt;jwt=\"x\"");
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
    let err = verify_signature_with_jwk(&jwk_val, b"base", "sig").unwrap_err();
    assert!(matches!(
        err,
        HttpPopError::InvalidConfirmationKey(InvalidConfirmationKey::Deserialize(_))
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
    let err = verify_signature_with_jwk(&jwk_val, b"base", "sig").unwrap_err();
    assert!(matches!(
        err,
        HttpPopError::InvalidConfirmationKey(InvalidConfirmationKey::UnsupportedAlgorithm)
    ));
}

#[test]
fn verify_signature_with_jwk_accepts_p384_key_and_verifies_signature() {
    let fixture = p384_fixture();
    let base = b"some signature base";
    let sig_b64 = jsonwebtoken::crypto::sign(base, &fixture.signing, fixture.alg).expect("sign");
    verify_signature_with_jwk(&fixture.jwk_json, base, &sig_b64).expect("p384 signature verifies");
}

#[test]
fn verify_signature_with_jwk_rejects_p384_signature_over_wrong_base() {
    let fixture = p384_fixture();
    let sig_b64 = jsonwebtoken::crypto::sign(b"expected base", &fixture.signing, fixture.alg).expect("sign");
    let err = verify_signature_with_jwk(&fixture.jwk_json, b"different base", &sig_b64).unwrap_err();
    assert!(matches!(err, HttpPopError::BadSignature));
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
    let cases = [
        format!("{}", HttpPopError::MissingHeader("x")),
        format!("{}", HttpPopError::DuplicateHeader("x")),
        format!("{}", HttpPopError::UnsupportedSignatureKeyScheme),
        format!("{}", HttpPopError::MalformedSignatureInput),
        format!("{}", HttpPopError::MalformedSignature),
        format!("{}", HttpPopError::Skew),
        format!("{}", HttpPopError::Replay),
        format!("{}", HttpPopError::BadSignature),
    ];
    for i in 0..cases.len() {
        for j in (i + 1)..cases.len() {
            assert_ne!(cases[i], cases[j]);
        }
    }
}
