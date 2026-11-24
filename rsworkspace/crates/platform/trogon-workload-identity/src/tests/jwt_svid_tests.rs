use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P384_SHA384_FIXED_SIGNING, ECDSA_P521_SHA512_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, EncodingKey, crypto};
use p256::ecdsa::SigningKey;
use p256::pkcs8::EncodePrivateKey;
use rsa::RsaPrivateKey;
use rsa::traits::PublicKeyParts;
use serde_json::{Value, json};
use spiffe::TrustDomain;
use tracing::Level;
use trogon_std::log_capture::{CapturedEvents, LevelFilter};

use super::*;
use crate::jwt_algorithm::JwtSvidAlgorithm as Alg;
use crate::jwt_authority::AuthorityKind;

pub(crate) const NOW: u64 = 1_700_000_000;
pub(crate) const SUB: &str = "spiffe://example.test/service/api";
pub(crate) const AUD: &str = "https://gateway.example.test/";

#[derive(Debug)]
pub(crate) struct FixedClock(pub(crate) SystemTime);

impl JwtClock for FixedClock {
    fn now(&self) -> SystemTime {
        self.0
    }
}

pub(crate) struct TestKey {
    pub(crate) kid: &'static str,
    encoding: EncodingKey,
    pub(crate) jwk: Value,
}

fn rsa_key() -> &'static TestKey {
    static KEY: OnceLock<TestKey> = OnceLock::new();
    KEY.get_or_init(|| {
        let private = RsaPrivateKey::new(&mut rand_core::OsRng, 2048).unwrap();
        let der = rsa::pkcs1::EncodeRsaPrivateKey::to_pkcs1_der(&private).unwrap();
        TestKey {
            kid: "rsa-1",
            encoding: EncodingKey::from_rsa_der(der.as_bytes()),
            jwk: json!({
                "kty": "RSA", "use": "jwt-svid", "kid": "rsa-1",
                "n": URL_SAFE_NO_PAD.encode(private.n().to_bytes_be()),
                "e": URL_SAFE_NO_PAD.encode(private.e().to_bytes_be()),
            }),
        }
    })
}

pub(crate) fn ec_key(kid: &'static str) -> TestKey {
    let signing = SigningKey::random(&mut rand_core::OsRng);
    let der = signing.to_pkcs8_der().unwrap();
    let point = signing.verifying_key().to_encoded_point(false);
    TestKey {
        kid,
        encoding: EncodingKey::from_ec_der(der.as_bytes()),
        jwk: json!({
            "kty": "EC", "use": "jwt-svid", "kid": kid, "crv": "P-256",
            "x": URL_SAFE_NO_PAD.encode(point.x().unwrap()),
            "y": URL_SAFE_NO_PAD.encode(point.y().unwrap()),
        }),
    }
}

/// A P-521 key pair of aws-lc-rs, the signer of ES512 test tokens.
struct P521Key {
    kid: &'static str,
    pair: EcdsaKeyPair,
    jwk: Value,
}

fn ec_jwk(kid: &str, crv: &str, point: &[u8]) -> Value {
    let (x, y) = point[1..].split_at((point.len() - 1) / 2);
    json!({
        "kty": "EC", "use": "jwt-svid", "kid": kid, "crv": crv,
        "x": URL_SAFE_NO_PAD.encode(x), "y": URL_SAFE_NO_PAD.encode(y),
    })
}

fn p521_key(kid: &'static str) -> P521Key {
    let pair = EcdsaKeyPair::generate(&ECDSA_P521_SHA512_FIXED_SIGNING).unwrap();
    let jwk = ec_jwk(kid, "P-521", pair.public_key().as_ref());
    P521Key { kid, pair, jwk }
}

fn p384_jwk(kid: &str) -> Value {
    let pair = EcdsaKeyPair::generate(&ECDSA_P384_SHA384_FIXED_SIGNING).unwrap();
    ec_jwk(kid, "P-384", pair.public_key().as_ref())
}

fn es512_token_with(key: &P521Key, header: Value, claims: &Value) -> String {
    let input = format!("{}.{}", b64(&header), b64(claims));
    let signature = key.pair.sign(&SystemRandom::new(), input.as_bytes()).unwrap();
    format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature.as_ref()))
}

fn es512_token(key: &P521Key, claims: &Value) -> String {
    es512_token_with(key, json!({"alg": "ES512", "typ": "JWT", "kid": key.kid}), claims)
}

fn verifier_for_jwks(jwks: Vec<Value>) -> JwtSvidVerifier {
    let jwks = json!({ "keys": jwks }).to_string();
    let bundle = JwtBundle::from_jwks(TrustDomain::new("example.test").unwrap(), &jwks).unwrap();
    verifier_with(JwtBundleSet::new().with_bundle(bundle).unwrap())
}

pub(crate) fn bundle(trust_domain: &str, keys: &[&TestKey]) -> JwtBundle {
    let mut entries: Vec<Value> = keys.iter().map(|key| key.jwk.clone()).collect();
    entries.push(json!({"kty": "EC", "use": "x509-svid", "crv": "P-256", "x": "AA", "y": "AA"}));
    let jwks = json!({ "keys": entries }).to_string();
    JwtBundle::from_jwks(TrustDomain::new(trust_domain).unwrap(), &jwks).unwrap()
}

fn verifier_for(keys: &[&TestKey]) -> JwtSvidVerifier {
    let bundles = JwtBundleSet::new().with_bundle(bundle("example.test", keys)).unwrap();
    verifier_with(bundles)
}

fn verifier_with(bundles: JwtBundleSet) -> JwtSvidVerifier {
    JwtSvidVerifier::new(audience(AUD), JwtBundleSource::static_for_tests(bundles).unwrap())
        .with_clock(Arc::new(FixedClock(UNIX_EPOCH + Duration::from_secs(NOW))))
}

pub(crate) fn audience(value: &str) -> JwtAudience {
    JwtAudience::new(value).unwrap()
}

fn peer_evidence(expires_at: SystemTime) -> PeerEvidence {
    PeerEvidence {
        application_id: ApplicationId::new("reports").unwrap(),
        expires_at,
        deadline: tokio::time::Instant::now() + Duration::from_secs(3600),
        spiffe_id: spiffe::SpiffeId::new("spiffe://example.test/service/client").unwrap(),
        leaf_sha256: [7; 32],
    }
}

pub(crate) fn hop() -> ConfidentialHop {
    ConfidentialHop::mutual_tls(&peer_evidence(UNIX_EPOCH + Duration::from_secs(NOW + 3600)))
}

pub(crate) fn claims() -> Value {
    json!({"sub": SUB, "aud": [AUD], "exp": NOW + 120})
}

fn b64(value: &Value) -> String {
    URL_SAFE_NO_PAD.encode(value.to_string())
}

fn sign(header: Value, claims: &Value, key: &EncodingKey, algorithm: Algorithm) -> String {
    let input = format!("{}.{}", b64(&header), b64(claims));
    let signature = crypto::sign(input.as_bytes(), key, algorithm).unwrap();
    format!("{input}.{signature}")
}

fn token_with(key: &TestKey, alg: &str, algorithm: Algorithm, claims: &Value) -> String {
    sign(
        json!({"alg": alg, "typ": "JWT", "kid": key.kid}),
        claims,
        &key.encoding,
        algorithm,
    )
}

pub(crate) fn ec_token(key: &TestKey, claims: &Value) -> String {
    token_with(key, "ES256", Algorithm::ES256, claims)
}

fn refused(verifier: &JwtSvidVerifier, token: &str) -> JwtSvidError {
    verifier.verify(token, &hop()).unwrap_err()
}

#[test]
fn accepts_an_es256_token() {
    let key = ec_key("ec-1");
    let verified = verifier_for(&[&key])
        .verify(&ec_token(&key, &claims()), &hop())
        .unwrap();
    assert_eq!(verified.workload_id().as_spiffe_id().to_string(), SUB);
    assert_eq!(verified.audience(), AUD);
    assert_eq!(verified.expires_at(), UNIX_EPOCH + Duration::from_secs(NOW + 120));
}

#[test]
fn a_jwt_svid_is_a_bearer_token_that_verifies_again_and_ignores_any_confirmation_claim() {
    let key = ec_key("ec-1");
    let verifier = verifier_for(&[&key]);
    let token = ec_token(&key, &claims());
    assert!(verifier.verify(&token, &hop()).is_ok());
    assert!(
        verifier.verify(&token, &hop()).is_ok(),
        "a replayed token verifies again"
    );
    let bound = json!({"sub": SUB, "aud": [AUD], "exp": NOW + 120, "cnf": {"jkt": "another-holders-key"}});
    assert!(verifier.verify(&ec_token(&key, &bound), &hop()).is_ok());
}

#[test]
fn accepts_rs256_and_ps256_tokens() {
    let key = rsa_key();
    let verifier = verifier_for(&[key]);
    for (name, algorithm) in [("RS256", Algorithm::RS256), ("PS256", Algorithm::PS256)] {
        let token = token_with(key, name, algorithm, &claims());
        assert!(verifier.verify(&token, &hop()).is_ok(), "{name}");
    }
}

#[test]
fn accepts_a_token_without_kid_or_typ_when_a_bundle_key_fits() {
    let key = ec_key("ec-1");
    let token = sign(json!({"alg": "ES256"}), &claims(), &key.encoding, Algorithm::ES256);
    assert!(verifier_for(&[&key]).verify(&token, &hop()).is_ok());
}

#[test]
fn accepts_only_a_single_audience_and_refuses_a_token_scoped_to_several() {
    let key = ec_key("ec-1");
    for audience in [json!(AUD), json!([AUD])] {
        let claims = json!({"sub": SUB, "aud": audience, "exp": NOW + 120});
        assert!(verifier_for(&[&key]).verify(&ec_token(&key, &claims), &hop()).is_ok());
    }
    for audience in [json!(["https://other.example.test/", AUD]), json!([AUD, AUD])] {
        let claims = json!({"sub": SUB, "aud": audience, "exp": NOW + 120});
        assert_eq!(
            refused(&verifier_for(&[&key]), &ec_token(&key, &claims)),
            JwtSvidError::MultipleAudiences
        );
    }
    let empty = json!({"sub": SUB, "aud": [], "exp": NOW + 120});
    assert_eq!(
        refused(&verifier_for(&[&key]), &ec_token(&key, &empty)),
        JwtSvidError::MissingAudience
    );
}

#[test]
fn refuses_a_wrong_audience() {
    let key = ec_key("ec-1");
    let claims = json!({"sub": SUB, "aud": ["https://other.example.test/"], "exp": NOW + 120});
    let error = refused(&verifier_for(&[&key]), &ec_token(&key, &claims));
    assert_eq!(error, JwtSvidError::AudienceMismatch);
}

#[test]
fn refuses_a_missing_audience() {
    let key = ec_key("ec-1");
    let claims = json!({"sub": SUB, "exp": NOW + 120});
    assert_eq!(
        refused(&verifier_for(&[&key]), &ec_token(&key, &claims)),
        JwtSvidError::MissingAudience
    );
}

#[test]
fn an_audience_that_is_a_bare_trust_domain_cannot_be_built() {
    for value in [
        "example.test",
        "EXAMPLE.test",
        "production",
        "spiffe://example.test",
        "spiffe://example.test/",
        "SPIFFE://example.test//",
        "spiffe://other.test",
    ] {
        assert_eq!(
            JwtAudience::new(value).unwrap_err(),
            JwtVerifierError::AudienceIsTrustDomain,
            "{value}"
        );
    }
}

#[test]
fn an_audience_that_is_empty_or_malformed_cannot_be_built() {
    for value in ["", "has space", "tab\t", "spiffe://example.test/a\nb"] {
        assert_eq!(
            JwtAudience::new(value).unwrap_err(),
            JwtVerifierError::AudienceInvalid,
            "{value:?}"
        );
    }
    let too_long = format!("https://example.test/{}", "a".repeat(2048));
    assert_eq!(
        JwtAudience::new(&too_long).unwrap_err(),
        JwtVerifierError::AudienceInvalid
    );
}

#[test]
fn an_audience_that_names_no_specific_service_cannot_be_built() {
    for value in [
        "https://",
        "https:///path",
        "://host/path",
        "1http://host/",
        "ht tp://host",
        "urn:example:service",
        "a_b",
    ] {
        assert!(JwtAudience::new(value).is_err(), "{value}");
    }
    assert_eq!(
        JwtAudience::new("https://").unwrap_err(),
        JwtVerifierError::AudienceNotSpecific
    );
}

#[test]
fn an_audience_naming_one_service_is_accepted() {
    for value in [
        "spiffe://example.test/service/gateway",
        "spiffe://prod.example.com/reports",
        "https://gateway.example.test/",
        "https://gateway.example.test",
    ] {
        assert_eq!(JwtAudience::new(value).unwrap().as_str(), value);
    }
}

#[test]
fn refuses_to_construct_without_bundles_or_with_bad_bounds() {
    let key = ec_key("ec-1");
    assert_eq!(
        JwtBundleSource::static_for_tests(JwtBundleSet::new()).unwrap_err(),
        JwtVerifierError::NoBundles
    );
    let verifier = verifier_for(&[&key]);
    assert_eq!(
        verifier.clone().with_max_lifetime(Duration::ZERO).unwrap_err(),
        JwtVerifierError::LifetimeCeilingInvalid
    );
    assert_eq!(
        verifier
            .clone()
            .with_max_lifetime(Duration::from_secs(3601))
            .unwrap_err(),
        JwtVerifierError::LifetimeCeilingInvalid
    );
    assert_eq!(
        verifier.with_leeway(Duration::from_secs(61)).unwrap_err(),
        JwtVerifierError::LeewayInvalid
    );
}

#[test]
fn refuses_an_expired_token() {
    let key = ec_key("ec-1");
    let claims = json!({"sub": SUB, "aud": [AUD], "exp": NOW - 31});
    assert_eq!(
        refused(&verifier_for(&[&key]), &ec_token(&key, &claims)),
        JwtSvidError::Expired
    );
}

#[test]
fn tolerates_only_the_bounded_leeway_after_expiry() {
    let key = ec_key("ec-1");
    let claims = json!({"sub": SUB, "aud": [AUD], "exp": NOW - 10});
    assert!(verifier_for(&[&key]).verify(&ec_token(&key, &claims), &hop()).is_ok());
}

#[test]
fn refuses_a_missing_expiry() {
    let key = ec_key("ec-1");
    let claims = json!({"sub": SUB, "aud": [AUD]});
    assert_eq!(
        refused(&verifier_for(&[&key]), &ec_token(&key, &claims)),
        JwtSvidError::MissingExpiry
    );
}

#[test]
fn refuses_a_lifetime_over_the_ceiling() {
    let key = ec_key("ec-1");
    let claims = json!({"sub": SUB, "aud": [AUD], "exp": NOW + 301});
    assert_eq!(
        refused(&verifier_for(&[&key]), &ec_token(&key, &claims)),
        JwtSvidError::LifetimeTooLong
    );
    let at_ceiling = json!({"sub": SUB, "aud": [AUD], "exp": NOW + 300});
    assert!(
        verifier_for(&[&key])
            .verify(&ec_token(&key, &at_ceiling), &hop())
            .is_ok()
    );
}

#[test]
fn a_configured_ceiling_replaces_the_default() {
    let key = ec_key("ec-1");
    let verifier = verifier_for(&[&key])
        .with_max_lifetime(Duration::from_secs(60))
        .unwrap();
    assert_eq!(
        refused(&verifier, &ec_token(&key, &claims())),
        JwtSvidError::LifetimeTooLong
    );
}

#[test]
fn refuses_a_token_that_is_not_yet_valid() {
    let key = ec_key("ec-1");
    let claims = json!({"sub": SUB, "aud": [AUD], "exp": NOW + 120, "nbf": NOW + 31});
    assert_eq!(
        refused(&verifier_for(&[&key]), &ec_token(&key, &claims)),
        JwtSvidError::NotYetValid
    );
}

#[test]
fn refuses_alg_none_before_any_key_lookup() {
    let key = ec_key("ec-1");
    let header = b64(&json!({"alg": "none", "typ": "JWT"}));
    let unsigned = format!("{header}.{}.", b64(&claims()));
    assert_eq!(
        refused(&verifier_for(&[&key]), &unsigned),
        JwtSvidError::AlgorithmNotAllowed
    );
    let garbage = format!("{header}.{}.AAAA", b64(&claims()));
    assert_eq!(
        refused(&verifier_for(&[&key]), &garbage),
        JwtSvidError::AlgorithmNotAllowed
    );
}

#[test]
fn refuses_hs256_signed_with_the_public_key_as_the_secret() {
    let key = ec_key("ec-1");
    let public_point = key.jwk["x"].as_str().unwrap().as_bytes();
    let confused = sign(
        json!({"alg": "HS256", "typ": "JWT", "kid": "ec-1"}),
        &claims(),
        &EncodingKey::from_secret(public_point),
        Algorithm::HS256,
    );
    assert_eq!(
        refused(&verifier_for(&[&key]), &confused),
        JwtSvidError::AlgorithmNotAllowed
    );
}

#[test]
fn refuses_eddsa_and_other_algorithms() {
    let key = ec_key("ec-1");
    for alg in ["EdDSA", "HS384", "HS512", "es256", "None", ""] {
        let token = format!("{}.{}.AAAA", b64(&json!({"alg": alg, "kid": "ec-1"})), b64(&claims()));
        assert_eq!(
            refused(&verifier_for(&[&key]), &token),
            JwtSvidError::AlgorithmNotAllowed,
            "{alg}"
        );
    }
    let missing = format!("{}.{}.AAAA", b64(&json!({"kid": "ec-1"})), b64(&claims()));
    assert_eq!(
        refused(&verifier_for(&[&key]), &missing),
        JwtSvidError::MissingAlgorithm
    );
}

#[test]
fn refuses_the_ml_dsa_algorithms_outside_the_nine_rsa_and_ecdsa_ones() {
    let key = ec_key("ec-1");
    for alg in ["ML-DSA-44", "ML-DSA-65", "ML-DSA-87"] {
        let token = format!(
            "{}.{}.AAAA",
            b64(&json!({"alg": alg, "typ": "JWT", "kid": "ec-1"})),
            b64(&claims())
        );
        assert_eq!(
            refused(&verifier_for(&[&key]), &token),
            JwtSvidError::AlgorithmNotAllowed,
            "{alg}"
        );
    }
}

#[test]
fn accepts_an_es512_token_signed_by_a_p521_key() {
    let key = p521_key("p521-1");
    let verifier = verifier_for_jwks(vec![key.jwk.clone(), p384_jwk("p384-1")]);
    let verified = verifier.verify(&es512_token(&key, &claims()), &hop()).unwrap();
    assert_eq!(verified.workload_id().as_spiffe_id().to_string(), SUB);
    let no_kid = es512_token_with(&key, json!({"alg": "ES512"}), &claims());
    assert!(verifier.verify(&no_kid, &hop()).is_ok());
}

#[test]
fn an_es512_token_goes_through_the_same_claim_and_header_checks() {
    let key = p521_key("p521-1");
    let verifier = verifier_for_jwks(vec![key.jwk.clone()]);
    let expired = json!({"sub": SUB, "aud": [AUD], "exp": NOW - 45});
    assert_eq!(refused(&verifier, &es512_token(&key, &expired)), JwtSvidError::Expired);
    let other_audience = json!({"sub": SUB, "aud": ["https://other.example.test/"], "exp": NOW + 120});
    assert_eq!(
        refused(&verifier, &es512_token(&key, &other_audience)),
        JwtSvidError::AudienceMismatch
    );
    let foreign = json!({"sub": "spiffe://other.test/service/api", "aud": [AUD], "exp": NOW + 120});
    assert_eq!(
        refused(&verifier, &es512_token(&key, &foreign)),
        JwtSvidError::UnknownTrustDomain
    );
    let unknown_kid = es512_token_with(&key, json!({"alg": "ES512", "kid": "nope"}), &claims());
    assert_eq!(refused(&verifier, &unknown_kid), JwtSvidError::UnknownKeyId);
    let extra = es512_token_with(&key, json!({"alg": "ES512", "kid": "p521-1", "jku": "x"}), &claims());
    assert_eq!(refused(&verifier, &extra), JwtSvidError::UnexpectedHeader);
}

#[test]
fn refuses_an_es512_token_whose_signature_does_not_verify() {
    let key = p521_key("p521-1");
    let verifier = verifier_for_jwks(vec![key.jwk.clone()]);
    let token = es512_token(&key, &claims());
    let (signing_input, signature) = token.rsplit_once('.').unwrap();
    let raw = URL_SAFE_NO_PAD.decode(signature).unwrap();
    assert_eq!(raw.len(), 132);
    let resign = |raw: &[u8]| format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(raw));

    let mut flipped = raw.clone();
    flipped[70] ^= 1;
    assert_eq!(refused(&verifier, &resign(&flipped)), JwtSvidError::InvalidSignature);
    assert_eq!(refused(&verifier, &resign(&raw[..131])), JwtSvidError::InvalidSignature);
    let mut long = raw.clone();
    long.push(0);
    assert_eq!(refused(&verifier, &resign(&long)), JwtSvidError::InvalidSignature);
    assert_eq!(refused(&verifier, &resign(&[0; 132])), JwtSvidError::InvalidSignature);

    let other_payload = es512_token(&key, &json!({"sub": SUB, "aud": [AUD], "exp": NOW + 121}));
    let swapped = format!("{}.{signature}", other_payload.rsplit_once('.').unwrap().0);
    assert_eq!(refused(&verifier, &swapped), JwtSvidError::InvalidSignature);

    let impostor = p521_key("p521-1");
    assert_eq!(
        refused(&verifier, &es512_token(&impostor, &claims())),
        JwtSvidError::InvalidSignature
    );
}

#[test]
fn refuses_es512_with_a_p256_or_p384_key_and_es256_or_es384_with_a_p521_key() {
    let p521 = p521_key("p521-1");
    let p256 = ec_key("p256-1");
    let p384 = p384_jwk("p384-1");
    let verifier = verifier_for_jwks(vec![p521.jwk.clone(), p256.jwk.clone(), p384]);
    for kid in ["p256-1", "p384-1"] {
        let token = es512_token_with(&p521, json!({"alg": "ES512", "kid": kid}), &claims());
        assert_eq!(refused(&verifier, &token), JwtSvidError::KeyAlgorithmMismatch, "{kid}");
    }
    for alg in ["ES256", "ES384"] {
        let token = es512_token_with(&p521, json!({"alg": alg, "kid": "p521-1"}), &claims());
        assert_eq!(refused(&verifier, &token), JwtSvidError::KeyAlgorithmMismatch, "{alg}");
    }
    let only_p256 = verifier_for(&[&p256]);
    let no_kid = es512_token_with(&p521, json!({"alg": "ES512"}), &claims());
    assert_eq!(refused(&only_p256, &no_kid), JwtSvidError::KeyAlgorithmMismatch);
    let only_p521 = verifier_for_jwks(vec![p521.jwk.clone()]);
    let es256_no_kid = sign(json!({"alg": "ES256"}), &claims(), &p256.encoding, Algorithm::ES256);
    assert_eq!(refused(&only_p521, &es256_no_kid), JwtSvidError::KeyAlgorithmMismatch);
}

#[test]
fn verifies_the_rfc_7515_es512_example_signature() {
    let jwk = json!({
        "kty": "EC", "use": "jwt-svid", "kid": "rfc7515", "crv": "P-521",
        "x": "AekpBQ8ST8a8VcfVOTNl353vSrDCLLJXmPk06wTjxrrjcBpXp5EOnYG_NjFZ6OvLFV1jSfS9tsz4qUxcWceqwQGk",
        "y": "ADSmRA43Z1DSNx_RvcLI87cdL07l6jQyyBXMoxVg_l2Th-x3S1WDhjDly79ajL4Kkd0AZMaZmh9ubmf63e3kyMj2",
    });
    let jwks = json!({"keys": [jwk]}).to_string();
    let bundle = JwtBundle::from_jwks(TrustDomain::new("example.test").unwrap(), &jwks).unwrap();
    let authority = bundle.authority("rfc7515").unwrap();
    let signing_input = "eyJhbGciOiJFUzUxMiJ9.UGF5bG9hZA";
    let signature = "AdwMgeerwtHoh-l192l60hp9wAHZFVJbLfD_UxMi70cwnZOYaRI1bKPWROc-mZZqwqT2SI-KGDKB34XO0aw_7XdtAG8GaSwFKdCAPZgoXD2YBJZCPEX3xKpRwcdOO8KpEHwJjyqOgzDO7iKvU8vcnwNrmxYbSW9ERBXukOXolLzeO_Jn";
    assert!(authority.verifies(Alg::Es512, signing_input, signature));
    assert!(!authority.verifies(Alg::Es512, "eyJhbGciOiJFUzUxMiJ9.UGF5bG9hZB", signature));
    assert!(!authority.verifies(Alg::Es384, signing_input, signature));
}

#[test]
fn refuses_a_bad_typ_and_unexpected_header_parameters() {
    let key = ec_key("ec-1");
    let verifier = verifier_for(&[&key]);
    let header = |extra: Value| {
        let mut base = json!({"alg": "ES256", "kid": "ec-1"});
        base.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        sign(base, &claims(), &key.encoding, Algorithm::ES256)
    };
    assert_eq!(
        refused(&verifier, &header(json!({"typ": "at+jwt"}))),
        JwtSvidError::InvalidType
    );
    assert_eq!(
        refused(&verifier, &header(json!({"typ": "jwt"}))),
        JwtSvidError::InvalidType
    );
    assert!(verifier.verify(&header(json!({"typ": "JOSE"})), &hop()).is_ok());
    assert_eq!(
        refused(&verifier, &header(json!({"jku": "https://attacker.example/keys"}))),
        JwtSvidError::UnexpectedHeader
    );
    assert_eq!(
        refused(&verifier, &header(json!({"crit": ["b64"]}))),
        JwtSvidError::UnexpectedHeader
    );
}

#[test]
fn refuses_a_key_from_another_trust_domains_bundle() {
    let trusted = ec_key("k1");
    let foreign = ec_key("k1");
    let bundles = JwtBundleSet::new()
        .with_bundle(bundle("example.test", &[&trusted]))
        .unwrap()
        .with_bundle(bundle("other.test", &[&foreign]))
        .unwrap();
    let verifier = verifier_with(bundles);
    // Signed by the key of other.test, claiming a workload of example.test.
    assert_eq!(
        refused(&verifier, &ec_token(&foreign, &claims())),
        JwtSvidError::InvalidSignature
    );
    // The same key is accepted for its own trust domain.
    let own = json!({"sub": "spiffe://other.test/service/api", "aud": [AUD], "exp": NOW + 120});
    assert!(verifier.verify(&ec_token(&foreign, &own), &hop()).is_ok());
}

#[test]
fn refuses_an_unknown_trust_domain() {
    let key = ec_key("ec-1");
    let claims = json!({"sub": "spiffe://unknown.test/service/api", "aud": [AUD], "exp": NOW + 120});
    assert_eq!(
        refused(&verifier_for(&[&key]), &ec_token(&key, &claims)),
        JwtSvidError::UnknownTrustDomain
    );
}

#[test]
fn refuses_an_unknown_kid_and_a_key_that_does_not_fit_alg() {
    let key = ec_key("ec-1");
    let verifier = verifier_for(&[&key]);
    let unknown = sign(
        json!({"alg": "ES256", "kid": "nope"}),
        &claims(),
        &key.encoding,
        Algorithm::ES256,
    );
    assert_eq!(refused(&verifier, &unknown), JwtSvidError::UnknownKeyId);
    let rsa = rsa_key();
    let wrong_kind = sign(
        json!({"alg": "RS256", "kid": "ec-1"}),
        &claims(),
        &rsa.encoding,
        Algorithm::RS256,
    );
    assert_eq!(refused(&verifier, &wrong_kind), JwtSvidError::KeyAlgorithmMismatch);
}

#[test]
fn refuses_a_tampered_payload() {
    let key = ec_key("ec-1");
    let token = ec_token(&key, &claims());
    let parts: Vec<&str> = token.split('.').collect();
    let forged = json!({"sub": "spiffe://example.test/service/admin", "aud": [AUD], "exp": NOW + 120});
    let tampered = format!("{}.{}.{}", parts[0], b64(&forged), parts[2]);
    assert_eq!(
        refused(&verifier_for(&[&key]), &tampered),
        JwtSvidError::InvalidSignature
    );
}

#[test]
fn refuses_a_subject_that_is_not_a_canonical_workload_id() {
    let key = ec_key("ec-1");
    let verifier = verifier_for(&[&key]);
    for sub in [
        "spiffe://example.test",
        "spiffe://example.test/",
        "spiffe://EXAMPLE.test/service/api",
        "spiffe://example.test/service/",
        "not-a-spiffe-id",
        "",
    ] {
        let claims = json!({"sub": sub, "aud": [AUD], "exp": NOW + 120});
        assert_eq!(
            refused(&verifier, &ec_token(&key, &claims)),
            JwtSvidError::InvalidSubject,
            "{sub:?}"
        );
    }
    let numeric = json!({"sub": 7, "aud": [AUD], "exp": NOW + 120});
    assert_eq!(
        refused(&verifier, &ec_token(&key, &numeric)),
        JwtSvidError::InvalidSubject
    );
    let missing = json!({"aud": [AUD], "exp": NOW + 120});
    assert_eq!(
        refused(&verifier, &ec_token(&key, &missing)),
        JwtSvidError::InvalidSubject
    );
}

#[test]
fn refuses_the_json_serialization_and_non_compact_shapes() {
    let key = ec_key("ec-1");
    let verifier = verifier_for(&[&key]);
    let token = ec_token(&key, &claims());
    let parts: Vec<&str> = token.split('.').collect();
    let flattened = json!({"protected": parts[0], "payload": parts[1], "signature": parts[2]}).to_string();
    assert_eq!(refused(&verifier, &flattened), JwtSvidError::JsonSerialization);
    let general = json!({"payload": parts[1], "signatures": [{"protected": parts[0], "signature": parts[2]}]});
    assert_eq!(
        refused(&verifier, &general.to_string()),
        JwtSvidError::JsonSerialization
    );
    assert_eq!(
        refused(&verifier, &format!("{token}.extra")),
        JwtSvidError::NotCompactSerialization
    );
    assert_eq!(
        refused(&verifier, &format!("{}.{}", parts[0], parts[1])),
        JwtSvidError::NotCompactSerialization
    );
    assert_eq!(refused(&verifier, ""), JwtSvidError::NotCompactSerialization);
    assert_eq!(
        refused(&verifier, &format!(".{}.{}", parts[1], parts[2])),
        JwtSvidError::NotCompactSerialization
    );
}

#[test]
fn refuses_segments_that_are_not_canonical_base64url() {
    let key = ec_key("ec-1");
    let verifier = verifier_for(&[&key]);
    let token = ec_token(&key, &claims());
    let parts: Vec<&str> = token.split('.').collect();
    for bad in [
        format!("{}.{}=.{}", parts[0], parts[1], parts[2]),
        format!("{}.{}.{}+", parts[0], parts[1], parts[2]),
        format!("{}.{}.", parts[0], parts[1]),
        format!("{}!.{}.{}", parts[0], parts[1], parts[2]),
    ] {
        assert_eq!(refused(&verifier, &bad), JwtSvidError::MalformedSegment, "{bad}");
    }
    let oversized = "a".repeat(crate::constants::MAX_JWT_SVID_BYTES + 1);
    assert_eq!(refused(&verifier, &oversized), JwtSvidError::TokenTooLarge);
}

#[test]
fn builds_bundles_only_from_valid_jwt_svid_keys() {
    let key = ec_key("ec-1");
    let td = || TrustDomain::new("example.test").unwrap();
    let from = |keys: Vec<Value>| JwtBundle::from_jwks(td(), &json!({"keys": keys}).to_string());
    assert_eq!(from(vec![]).unwrap_err(), JwtBundleError::NoAuthorities);
    let x509_only = json!({"kty": "EC", "use": "x509-svid", "kid": "x", "crv": "P-256", "x": "AA", "y": "AA"});
    assert_eq!(from(vec![x509_only]).unwrap_err(), JwtBundleError::NoAuthorities);
    let mut no_kid = key.jwk.clone();
    no_kid.as_object_mut().unwrap().remove("kid");
    assert_eq!(from(vec![no_kid]).unwrap_err(), JwtBundleError::MissingKeyId);
    assert_eq!(
        from(vec![key.jwk.clone(), key.jwk.clone()]).unwrap_err(),
        JwtBundleError::DuplicateKeyId
    );
    let mut p521_label = key.jwk.clone();
    p521_label["crv"] = json!("P-521");
    assert_eq!(from(vec![p521_label]).unwrap_err(), JwtBundleError::UnsupportedKey);
    let mut unknown_curve = key.jwk.clone();
    unknown_curve["crv"] = json!("P-999");
    assert_eq!(from(vec![unknown_curve]).unwrap_err(), JwtBundleError::UnsupportedKey);
    let small_rsa = json!({"kty": "RSA", "use": "jwt-svid", "kid": "r", "n": "AQAB", "e": "AQAB"});
    assert_eq!(from(vec![small_rsa]).unwrap_err(), JwtBundleError::UnsupportedKey);
    assert_eq!(JwtBundle::from_jwks(td(), "{").unwrap_err(), JwtBundleError::Malformed);
    let doubled = JwtBundleSet::new()
        .with_bundle(bundle("example.test", &[&key]))
        .unwrap()
        .with_bundle(bundle("example.test", &[&key]));
    assert_eq!(doubled.unwrap_err(), JwtBundleError::DuplicateTrustDomain);
}

#[test]
fn a_key_without_a_use_parameter_is_a_jwt_authority_as_a_spire_agent_sends_it() {
    let key = ec_key("ec-1");
    let mut bare = key.jwk.clone();
    bare.as_object_mut().unwrap().remove("use");
    let x509 = json!({"kty": "EC", "use": "x509-svid", "kid": "x", "crv": "P-256", "x": "AA", "y": "AA"});
    let jwks = json!({"keys": [bare, x509]}).to_string();
    let bundle = JwtBundle::from_jwks(TrustDomain::new("example.test").unwrap(), &jwks).unwrap();
    assert!(bundle.authority("ec-1").is_some());
    assert!(bundle.authority("x").is_none());
}

#[test]
fn a_bundle_refuses_oversized_documents_too_many_keys_and_malformed_ec_keys() {
    let td = || TrustDomain::new("example.test").unwrap();
    let oversized = " ".repeat(crate::constants::MAX_JWKS_BYTES + 1);
    assert_eq!(
        JwtBundle::from_jwks(td(), &oversized).unwrap_err(),
        JwtBundleError::TooLarge
    );
    let many: Vec<Value> = (0..=crate::constants::MAX_JWT_AUTHORITIES)
        .map(|index| {
            let mut jwk = ec_key("ec-1").jwk;
            jwk["kid"] = json!(format!("ec-{index}"));
            jwk
        })
        .collect();
    assert_eq!(
        JwtBundle::from_jwks(td(), &json!({"keys": many}).to_string()).unwrap_err(),
        JwtBundleError::TooManyAuthorities
    );
    let from = |jwk: Value| JwtBundle::from_jwks(td(), &json!({"keys": [jwk]}).to_string());
    let valid = ec_key("ec-1").jwk;
    let mut short = valid.clone();
    short["x"] = json!("AA");
    assert_eq!(from(short).unwrap_err(), JwtBundleError::UnsupportedKey);
    let mut no_x = valid.clone();
    no_x.as_object_mut().unwrap().remove("x");
    assert_eq!(from(no_x).unwrap_err(), JwtBundleError::UnsupportedKey);
    let mut okp = valid;
    okp["kty"] = json!("OKP");
    assert_eq!(from(okp).unwrap_err(), JwtBundleError::UnsupportedKey);
}

#[test]
fn an_ec_key_of_any_curve_is_accepted_only_with_a_point_on_that_curve() {
    let td = || TrustDomain::new("example.test").unwrap();
    let from = |jwk: Value| JwtBundle::from_jwks(td(), &json!({"keys": [jwk]}).to_string());
    for (curve, valid, width) in [
        ("P-256", ec_key("ec-1").jwk, 32),
        ("P-384", p384_jwk("p384-1"), 48),
        ("P-521", p521_key("p521-1").jwk, 66),
    ] {
        assert!(from(valid.clone()).is_ok(), "{curve}");
        let coordinate = |jwk: &Value, name: &str| URL_SAFE_NO_PAD.decode(jwk[name].as_str().unwrap()).unwrap();
        let with = |name: &str, bytes: &[u8]| {
            let mut jwk = valid.clone();
            jwk[name] = json!(URL_SAFE_NO_PAD.encode(bytes));
            jwk
        };
        for name in ["x", "y"] {
            let full = coordinate(&valid, name);
            assert_eq!(full.len(), width, "{curve}");
            assert_eq!(
                from(with(name, &full[1..])).unwrap_err(),
                JwtBundleError::UnsupportedKey,
                "{curve} {name} too short"
            );
            let mut long = full.clone();
            long.push(0);
            assert_eq!(
                from(with(name, &long)).unwrap_err(),
                JwtBundleError::UnsupportedKey,
                "{curve} {name} too long"
            );
            let mut off_curve = full.clone();
            off_curve[width - 1] ^= 1;
            assert_eq!(
                from(with(name, &off_curve)).unwrap_err(),
                JwtBundleError::UnsupportedKey,
                "{curve} {name} off curve"
            );
        }
        let mut zero = valid.clone();
        zero["x"] = json!(URL_SAFE_NO_PAD.encode(vec![0; width]));
        zero["y"] = json!(URL_SAFE_NO_PAD.encode(vec![0; width]));
        assert_eq!(
            from(zero).unwrap_err(),
            JwtBundleError::UnsupportedKey,
            "{curve} zero point"
        );
        let mut no_y = valid;
        no_y.as_object_mut().unwrap().remove("y");
        assert_eq!(from(no_y).unwrap_err(), JwtBundleError::UnsupportedKey, "{curve} no y");
    }
}

#[test]
fn a_p384_key_accepts_only_es384_and_every_algorithm_maps_to_its_backend() {
    assert!(AuthorityKind::EcP384.accepts(Alg::Es384));
    assert!(!AuthorityKind::EcP384.accepts(Alg::Es256));
    assert!(!AuthorityKind::EcP256.accepts(Alg::Es384));
    assert!(AuthorityKind::EcP521.accepts(Alg::Es512));
    for other in [Alg::Es256, Alg::Es384, Alg::Rs512, Alg::Ps512] {
        assert!(!AuthorityKind::EcP521.accepts(other));
    }
    assert!(!AuthorityKind::EcP256.accepts(Alg::Es512));
    assert!(!AuthorityKind::EcP384.accepts(Alg::Es512));
    assert!(!AuthorityKind::Rsa.accepts(Alg::Es512));
    for (name, rsa, backend) in [
        ("RS256", true, Some(Algorithm::RS256)),
        ("RS384", true, Some(Algorithm::RS384)),
        ("RS512", true, Some(Algorithm::RS512)),
        ("ES256", false, Some(Algorithm::ES256)),
        ("ES384", false, Some(Algorithm::ES384)),
        ("ES512", false, None),
        ("PS256", true, Some(Algorithm::PS256)),
        ("PS384", true, Some(Algorithm::PS384)),
        ("PS512", true, Some(Algorithm::PS512)),
    ] {
        let algorithm = Alg::parse(name).unwrap();
        assert_eq!(algorithm.is_rsa(), rsa, "{name}");
        assert_eq!(algorithm.backend(), backend, "{name}");
    }
}

#[test]
fn the_system_clock_is_after_the_epoch() {
    assert!(SystemJwtClock.now() > UNIX_EPOCH);
}

#[test]
fn a_widened_leeway_accepts_a_token_expired_within_it() {
    let key = ec_key("ec-1");
    let expired = json!({"sub": SUB, "aud": [AUD], "exp": NOW - 45});
    let token = ec_token(&key, &expired);
    assert_eq!(refused(&verifier_for(&[&key]), &token), JwtSvidError::Expired);
    let wide = verifier_for(&[&key]).with_leeway(Duration::from_secs(60)).unwrap();
    assert!(wide.verify(&token, &hop()).is_ok());
}

#[test]
fn refuses_audiences_and_dates_of_the_wrong_shape() {
    let key = ec_key("ec-1");
    let verifier = verifier_for(&[&key]);
    for aud in [json!([AUD, 7]), json!(7)] {
        let token = ec_token(&key, &json!({"sub": SUB, "aud": aud, "exp": NOW + 120}));
        assert_eq!(refused(&verifier, &token), JwtSvidError::MalformedClaims);
    }
    for exp in [json!("soon"), json!(-1.5)] {
        let token = ec_token(&key, &json!({"sub": SUB, "aud": [AUD], "exp": exp}));
        assert_eq!(refused(&verifier, &token), JwtSvidError::MalformedClaims);
    }
}

#[test]
fn a_fractional_expiry_is_rounded_down() {
    let key = ec_key("ec-1");
    let token = ec_token(
        &key,
        &json!({"sub": SUB, "aud": [AUD], "exp": (NOW + 120) as f64 + 0.9}),
    );
    let verified = verifier_for(&[&key]).verify(&token, &hop()).unwrap();
    assert_eq!(verified.expires_at(), UNIX_EPOCH + Duration::from_secs(NOW + 120));
}

#[test]
fn refuses_when_no_bundle_key_fits_a_token_without_a_kid() {
    let key = ec_key("ec-1");
    let verifier = verifier_for(&[&key]);
    let rsa = rsa_key();
    let token = sign(json!({"alg": "RS256"}), &claims(), &rsa.encoding, Algorithm::RS256);
    assert_eq!(refused(&verifier, &token), JwtSvidError::KeyAlgorithmMismatch);
}

#[test]
fn refuses_headers_and_payloads_of_the_wrong_shape() {
    let key = ec_key("ec-1");
    let verifier = verifier_for(&[&key]);
    for (header, expected) in [
        (json!({"alg": 5}), JwtSvidError::AlgorithmNotAllowed),
        (json!({"alg": "ES256", "kid": ""}), JwtSvidError::MalformedHeader),
        (json!({"alg": "ES256", "kid": 7}), JwtSvidError::MalformedHeader),
    ] {
        let token = sign(header, &claims(), &key.encoding, Algorithm::ES256);
        assert_eq!(refused(&verifier, &token), expected);
    }
    let not_an_object = format!("{}.{}.AA", b64(&json!({"alg": "ES256"})), b64(&json!([1])));
    assert_eq!(refused(&verifier, &not_an_object), JwtSvidError::MalformedClaims);
}

#[test]
fn refusals_emit_warn_events_with_stable_reasons_and_no_token_material() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::WARN);
    let key = ec_key("ec-1");
    let verifier = verifier_for(&[&key]);
    let expired = json!({"sub": SUB, "aud": [AUD], "exp": NOW - 100});
    let cases = [
        (ec_token(&key, &expired), "expired"),
        (
            ec_token(&key, &json!({"sub": SUB, "exp": NOW + 60})),
            "missing_audience",
        ),
        (
            ec_token(&key, &json!({"sub": SUB, "aud": ["x"], "exp": NOW + 60})),
            "audience_mismatch",
        ),
        ("a.b".to_owned(), "not_compact_serialization"),
        (
            format!("{}.{}.AAAA", b64(&json!({"alg": "none"})), b64(&claims())),
            "algorithm_not_allowed",
        ),
    ];
    let mut secrets = Vec::new();
    for (token, _) in &cases {
        verifier.verify(token, &hop()).unwrap_err();
        secrets.extend(token.split('.').map(str::to_owned));
    }
    assert!(verifier.verify(&ec_token(&key, &claims()), &hop()).is_ok());
    let expected: Vec<&str> = cases.iter().map(|(_, reason)| *reason).collect();
    let events = captured.at(Level::WARN);
    let reasons: Vec<&str> = events.iter().filter_map(|event| event.field("reason")).collect();
    assert_eq!(reasons, expected);
    let text = format!("{:?}", captured.events());
    for secret in secrets.iter().filter(|segment| segment.len() > 8) {
        assert!(!text.contains(secret.as_str()));
    }
}

#[test]
fn every_refusal_has_a_distinct_snake_case_reason_code() {
    let all = [
        JwtSvidError::BundlesNotDelivered,
        JwtSvidError::BundlesStale,
        JwtSvidError::TokenTooLarge,
        JwtSvidError::JsonSerialization,
        JwtSvidError::NotCompactSerialization,
        JwtSvidError::MalformedSegment,
        JwtSvidError::MalformedHeader,
        JwtSvidError::UnexpectedHeader,
        JwtSvidError::MissingAlgorithm,
        JwtSvidError::AlgorithmNotAllowed,
        JwtSvidError::InvalidType,
        JwtSvidError::MalformedClaims,
        JwtSvidError::InvalidSubject,
        JwtSvidError::UnknownTrustDomain,
        JwtSvidError::UnknownKeyId,
        JwtSvidError::KeyAlgorithmMismatch,
        JwtSvidError::InvalidSignature,
        JwtSvidError::MissingAudience,
        JwtSvidError::AudienceMismatch,
        JwtSvidError::MultipleAudiences,
        JwtSvidError::MissingExpiry,
        JwtSvidError::Expired,
        JwtSvidError::NotYetValid,
        JwtSvidError::LifetimeTooLong,
        JwtSvidError::HopExpired,
    ];
    let codes: std::collections::BTreeSet<&str> = all.iter().map(|error| error.code()).collect();
    assert_eq!(codes.len(), all.len());
    assert!(
        codes
            .iter()
            .all(|code| code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
    );
}

#[test]
fn the_default_lifetime_ceiling_is_five_minutes_and_the_configurable_maximum_one_hour() {
    assert_eq!(
        crate::constants::DEFAULT_JWT_SVID_MAX_LIFETIME,
        Duration::from_secs(300)
    );
    assert_eq!(
        crate::constants::MAX_JWT_SVID_LIFETIME_CEILING,
        Duration::from_secs(3600)
    );
}

#[test]
fn a_verified_token_records_a_mutual_tls_hop() {
    let key = ec_key("ec-1");
    let verified = verifier_for(&[&key])
        .verify(&ec_token(&key, &claims()), &hop())
        .unwrap();
    assert_eq!(verified.hop(), HopKind::MutualTls);
    assert_eq!(verified.hop().as_str(), "mutual_tls");
    assert!(hop().peer_evidence().is_some());
}

#[test]
fn a_mutual_tls_hop_whose_peer_evidence_has_expired_is_refused() {
    let key = ec_key("ec-1");
    let expired = ConfidentialHop::mutual_tls(&peer_evidence(UNIX_EPOCH + Duration::from_secs(NOW)));
    assert_eq!(
        verifier_for(&[&key])
            .verify(&ec_token(&key, &claims()), &expired)
            .unwrap_err(),
        JwtSvidError::HopExpired
    );
}

#[tokio::test]
async fn a_verified_token_records_a_unix_socket_hop() {
    let key = ec_key("ec-1");
    let (local, _remote) = tokio::net::UnixStream::pair().unwrap();
    let hop = ConfidentialHop::unix_socket(&local).unwrap();
    assert!(hop.peer_evidence().is_none());
    assert!(hop.unix_peer().is_some());
    let verified = verifier_for(&[&key]).verify(&ec_token(&key, &claims()), &hop).unwrap();
    assert_eq!(verified.hop(), HopKind::UnixSocket);
    assert_eq!(verified.hop().as_str(), "unix_socket");
}
