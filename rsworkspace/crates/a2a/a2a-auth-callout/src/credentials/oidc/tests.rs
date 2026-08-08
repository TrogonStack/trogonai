use super::*;
use crate::jwt::SigningKey;
use crate::signing_key_source::{KeyVersion, SigningKeyHandle};
use std::time::Duration;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::jwk::{
    AlgorithmParameters, CommonParameters, EllipticCurveKeyParameters, EllipticCurveKeyType, Jwk, KeyOperations,
    PublicKeyUse, RSAKeyParameters, RSAKeyType,
};
use rand_core::OsRng;
use rsa::RsaPrivateKey;
use rsa::pkcs8::EncodePrivateKey;
use rsa::traits::PublicKeyParts;
use serde::Serialize;

fn b64url_uint_be(bytes: &[u8]) -> String {
    let start = bytes
        .iter()
        .position(|&b| b != 0)
        .unwrap_or(bytes.len().saturating_sub(1));
    let trimmed = if start >= bytes.len() {
        &bytes[bytes.len().saturating_sub(1)..]
    } else {
        &bytes[start..]
    };
    URL_SAFE_NO_PAD.encode(trimmed)
}

fn test_jwks_and_encoding_key(rng: &mut OsRng) -> (JwkSet, jsonwebtoken::EncodingKey) {
    let key = RsaPrivateKey::new(rng, 2048).expect("rsa key");
    let encoding_key =
        jsonwebtoken::EncodingKey::from_rsa_pem(key.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF).expect("pem").as_bytes())
            .expect("encoding key");
    let public = key.to_public_key();
    let n = b64url_uint_be(&public.n().to_bytes_be());
    let e = b64url_uint_be(&public.e().to_bytes_be());
    let jwk = Jwk {
        common: CommonParameters {
            public_key_use: Some(PublicKeyUse::Signature),
            // `verify`, not `sign`: this is the *public* half of the pair, and
            // RFC 7517 section 4.3 scopes `key_ops` to what this key can do.
            key_operations: Some(vec![KeyOperations::Verify]),
            key_id: Some("test-kid".into()),
            x509_url: None,
            x509_chain: None,
            x509_sha1_fingerprint: None,
            x509_sha256_fingerprint: None,
            ..Default::default()
        },
        algorithm: AlgorithmParameters::RSA(RSAKeyParameters {
            key_type: RSAKeyType::RSA,
            n,
            e,
        }),
    };
    (JwkSet { keys: vec![jwk] }, encoding_key)
}

#[test]
fn rejects_empty_audience_config() {
    let rng = &mut OsRng;
    let (jwks, _) = test_jwks_and_encoding_key(rng);
    let v = JwksOidcVerifier::with_static_jwks(OidcIssuerUrl::parse("https://issuer.example").unwrap(), vec![], jwks);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let err = rt
        .block_on(v.verify_internal(&BearerToken::new("x.y.z"), &AudienceAccount::new("acct")))
        .unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(_))
    ));
}

#[tokio::test]
async fn verify_happy_path_rs256() {
    let rng = &mut OsRng;
    let (jwks, enc) = test_jwks_and_encoding_key(rng);
    let issuer = OidcIssuerUrl::parse("https://issuer.example").unwrap();
    let verifier = JwksOidcVerifier::with_static_jwks(issuer.clone(), vec!["a2a-client".into()], jwks);
    #[derive(Serialize)]
    struct IdClaims {
        sub: String,
        iss: String,
        aud: String,
        exp: u64,
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let id = IdClaims {
        sub: "user-42".into(),
        iss: issuer.as_str().to_owned(),
        aud: "a2a-client".into(),
        exp: now + 600,
    };
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("test-kid".into());
    let token = jsonwebtoken::encode(&header, &id, &enc).expect("encode");
    let account = AudienceAccount::new("nats-acct-1");
    let user = verifier
        .verify_internal(&BearerToken::new(token), &account)
        .await
        .unwrap();
    assert_eq!(user.sub.as_str(), "user-42");
    assert_eq!(user.aud.as_str(), "nats-acct-1");
    assert!(!user.caller_id.as_str().contains('.'));
    let issuer = nkeys::KeyPair::new_account();
    let issuer_seed = issuer.seed().expect("issuer seed");
    let subject_kp = nkeys::KeyPair::new_user();
    let handle = SigningKeyHandle::new(
        KeyVersion::new("test").unwrap(),
        SigningKey::from_seed(&issuer_seed).unwrap(),
    );
    let mut user = user;
    user.kid = handle.version().clone();
    let subject =
        crate::jwt::UserJwtSubject::from_user_nkey(crate::wire::NkeyPublic::parse(subject_kp.public_key()).unwrap());
    let minted = user
        .mint(
            &handle.minting_material(),
            &subject,
            std::time::SystemTime::now(),
            Duration::from_secs(60),
        )
        .unwrap();
    assert!(minted.as_str().split('.').count() == 3);
}

#[tokio::test]
async fn verify_fails_bad_signature() {
    let rng = &mut OsRng;
    let (jwks, enc) = test_jwks_and_encoding_key(rng);
    let issuer = OidcIssuerUrl::parse("https://issuer.example").unwrap();
    let verifier = JwksOidcVerifier::with_static_jwks(issuer.clone(), vec!["a2a-client".into()], jwks);
    #[derive(Serialize)]
    struct IdClaims {
        sub: String,
        iss: String,
        aud: String,
        exp: u64,
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let id = IdClaims {
        sub: "user-42".into(),
        iss: issuer.as_str().to_owned(),
        aud: "a2a-client".into(),
        exp: now + 600,
    };
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("test-kid".into());
    let token = jsonwebtoken::encode(&header, &id, &enc).expect("encode");
    let mut parts: Vec<String> = token.split('.').map(String::from).collect();
    {
        let sig = &mut parts[2];
        if let Some(mut c) = sig.pop() {
            c = if c == 'A' { 'B' } else { 'A' };
            sig.push(c);
        }
    }
    let bad = parts.join(".");
    let err = verifier
        .verify_internal(&BearerToken::new(bad), &AudienceAccount::new("acct"))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(_))
    ));
}

#[tokio::test]
async fn discover_fetches_jwks_via_wiremock() {
    let mock_srv = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/.well-known/openid-configuration"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(
            format!(
                r#"{{"issuer":"{}","jwks_uri":"{}/jwks"}}"#,
                mock_srv.uri(),
                mock_srv.uri()
            ),
            "application/json",
        ))
        .mount(&mock_srv)
        .await;
    let jwk_body = serde_json::json!({"keys":[]});
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/jwks"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(jwk_body.to_string(), "application/json"))
        .mount(&mock_srv)
        .await;
    let issuer = OidcIssuerUrl::parse(mock_srv.uri()).unwrap();
    let v = JwksOidcVerifier::discover(issuer, vec!["aud".into()])
        .await
        .expect("discover");
    let jwks = v.fetch_jwks().await.expect("jwks");
    assert!(jwks.keys.is_empty());
}

#[tokio::test]
async fn discover_rejects_jwks_uri_outside_issuer_origin() {
    let mock_srv = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/.well-known/openid-configuration"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(
            format!(
                r#"{{"issuer":"{}","jwks_uri":"https://attacker.example.com/jwks"}}"#,
                mock_srv.uri()
            ),
            "application/json",
        ))
        .mount(&mock_srv)
        .await;
    let issuer = OidcIssuerUrl::parse(mock_srv.uri()).unwrap();
    let res = JwksOidcVerifier::discover(issuer, vec!["aud".into()]).await;
    let Err(err) = res else {
        panic!("expected origin mismatch error");
    };
    let AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(msg)) = err else {
        panic!("expected jwks_uri origin mismatch");
    };
    assert_eq!(
        msg,
        format!(
            "OIDC jwks_uri \"https://attacker.example.com/jwks\" is outside issuer origin {:?}",
            mock_srv.uri()
        )
    );
}

#[tokio::test]
async fn discover_rejects_mismatched_issuer_claim() {
    let mock_srv = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/.well-known/openid-configuration"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(
            r#"{"issuer":"https://other.example.com","jwks_uri":"https://other.example.com/jwks"}"#,
            "application/json",
        ))
        .mount(&mock_srv)
        .await;
    let issuer = OidcIssuerUrl::parse(mock_srv.uri()).unwrap();
    let res = JwksOidcVerifier::discover(issuer, vec!["aud".into()]).await;
    let Err(err) = res else {
        panic!("expected issuer mismatch error");
    };
    let AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(msg)) = err else {
        panic!("expected issuer mismatch");
    };
    assert_eq!(
        msg,
        format!(
            "OIDC discovery issuer mismatch: configured={:?} discovered={:?}",
            mock_srv.uri(),
            "https://other.example.com"
        )
    );
}

#[test]
fn oidc_client_id_rejects_empty_and_whitespace() {
    assert!(OidcClientId::new("").is_err());
    assert!(OidcClientId::new("   ").is_err());
    assert!(OidcClientId::new("good-client").is_ok());
}

#[test]
fn same_origin_normalizes_default_ports() {
    assert!(super::same_origin(
        "https://idp.example.com/jwks",
        "https://idp.example.com:443"
    ));
    assert!(super::same_origin(
        "https://idp.example.com:443/jwks",
        "https://idp.example.com"
    ));
    assert!(super::same_origin(
        "http://idp.example.com:80/jwks",
        "http://idp.example.com"
    ));
    assert!(!super::same_origin(
        "https://idp.example.com:444/jwks",
        "https://idp.example.com"
    ));
    assert!(!super::same_origin(
        "http://idp.example.com/jwks",
        "https://idp.example.com"
    ));
}

#[test]
fn oidc_issuer_url_strips_trailing_slashes_and_rejects_empty() {
    let url = OidcIssuerUrl::parse("https://idp.example.com///").unwrap();
    assert_eq!(url.as_str(), "https://idp.example.com");

    let err = OidcIssuerUrl::parse("///").unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(_))
    ));

    let err_empty = OidcIssuerUrl::parse("").unwrap_err();
    assert!(matches!(
        err_empty,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(_))
    ));
}

#[test]
fn oidc_client_id_as_str_returns_value() {
    let id = OidcClientId::new("my-client").unwrap();
    assert_eq!(id.as_str(), "my-client");
}

#[test]
fn same_origin_returns_false_when_candidate_has_no_scheme() {
    // No "://" separator → url_origin returns None → same_origin returns false.
    assert!(!super::same_origin("no-scheme", "https://idp.example.com"));
}

#[test]
fn same_origin_returns_false_when_expected_has_no_scheme() {
    assert!(!super::same_origin("https://idp.example.com/jwks", "no-scheme"));
}

#[test]
fn same_origin_returns_false_for_empty_host() {
    // scheme://... with no host component → url_origin returns None.
    assert!(!super::same_origin("https:///path", "https://idp.example.com"));
}

#[tokio::test]
async fn verify_fails_with_non_rsa_jwk() {
    let issuer = OidcIssuerUrl::parse("https://issuer.example").unwrap();
    let ec_jwk = Jwk {
        common: CommonParameters {
            key_id: Some("ec-kid".into()),
            ..Default::default()
        },
        algorithm: AlgorithmParameters::EllipticCurve(EllipticCurveKeyParameters {
            key_type: EllipticCurveKeyType::EC,
            curve: jsonwebtoken::jwk::EllipticCurve::P256,
            x: "dummyx".into(),
            y: "dummyy".into(),
        }),
    };
    let jwks = JwkSet { keys: vec![ec_jwk] };
    let verifier = JwksOidcVerifier::with_static_jwks(issuer, vec!["aud".into()], jwks);

    // The kid resolves to the EC JWK, but ES256 is outside the deployment's
    // allow-list, so the token is refused on its algorithm before the key it
    // points at is ever considered. A header-only token is enough: nothing
    // past decode_header runs.
    let header_b64 =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"alg":"ES256","kid":"ec-kid","typ":"JWT"}"#);
    let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{}");
    let fake_token = format!("{header_b64}.{payload_b64}.sig");

    let err = verifier
        .verify_internal(&BearerToken::new(fake_token), &AudienceAccount::new("acct"))
        .await
        .unwrap_err();
    let AuthCalloutError::CredentialVerification(CredentialError::UnsupportedTokenAlgorithm { algorithm }) = err else {
        panic!("expected UnsupportedTokenAlgorithm, got {err:?}");
    };
    assert_eq!(algorithm, jsonwebtoken::Algorithm::ES256);
}

#[tokio::test]
async fn verify_fails_with_a_non_rsa_jwk_reached_under_an_allowed_algorithm() {
    let issuer = OidcIssuerUrl::parse("https://issuer.example").unwrap();
    let ec_jwk = Jwk {
        common: CommonParameters {
            key_id: Some("ec-kid".into()),
            ..Default::default()
        },
        algorithm: AlgorithmParameters::EllipticCurve(EllipticCurveKeyParameters {
            key_type: EllipticCurveKeyType::EC,
            curve: jsonwebtoken::jwk::EllipticCurve::P256,
            x: "dummyx".into(),
            y: "dummyy".into(),
        }),
    };
    let jwks = JwkSet { keys: vec![ec_jwk] };
    let verifier = JwksOidcVerifier::with_static_jwks(issuer, vec!["aud".into()], jwks);

    // RS256 is allow-listed and the EC JWK declares no purpose of its own, so
    // neither guard ahead of the key material refuses this token. What refuses
    // it is the verifier's own RSA-only support, which the guards must not be
    // allowed to mask.
    let header_b64 = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","kid":"ec-kid","typ":"JWT"}"#);
    let payload_b64 = URL_SAFE_NO_PAD.encode(b"{}");
    let fake_token = format!("{header_b64}.{payload_b64}.sig");

    let err = verifier
        .verify_internal(&BearerToken::new(fake_token), &AudienceAccount::new("acct"))
        .await
        .unwrap_err();
    let AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(message)) = err else {
        panic!("expected InvalidCredentials, got {err:?}");
    };
    assert!(message.contains("must be RSA"), "{message}");
}

#[tokio::test]
async fn verify_fails_with_an_rsa_jwk_whose_components_do_not_decode() {
    let issuer = OidcIssuerUrl::parse("https://issuer.example").unwrap();
    let broken_jwk = Jwk {
        common: CommonParameters {
            key_id: Some("broken-kid".into()),
            ..Default::default()
        },
        algorithm: AlgorithmParameters::RSA(RSAKeyParameters {
            key_type: RSAKeyType::RSA,
            n: "not base64url".into(),
            e: "AQAB".into(),
        }),
    };
    let jwks = JwkSet { keys: vec![broken_jwk] };
    let verifier = JwksOidcVerifier::with_static_jwks(issuer, vec!["aud".into()], jwks);

    let header_b64 = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","kid":"broken-kid","typ":"JWT"}"#);
    let payload_b64 = URL_SAFE_NO_PAD.encode(b"{}");
    let fake_token = format!("{header_b64}.{payload_b64}.sig");

    let err = verifier
        .verify_internal(&BearerToken::new(fake_token), &AudienceAccount::new("acct"))
        .await
        .unwrap_err();
    let AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(message)) = err else {
        panic!("expected InvalidCredentials, got {err:?}");
    };
    assert!(message.contains("invalid RSA JWK components"), "{message}");
}

#[tokio::test]
async fn oidc_verifier_trait_delegates_to_verify_internal() {
    // Exercise the OidcVerifier::verify blanket impl on JwksOidcVerifier.
    let rng = &mut OsRng;
    let (jwks, enc) = test_jwks_and_encoding_key(rng);
    let issuer = OidcIssuerUrl::parse("https://issuer.example").unwrap();
    let verifier: &dyn OidcVerifier =
        &JwksOidcVerifier::with_static_jwks(issuer.clone(), vec!["a2a-client".into()], jwks);
    #[derive(Serialize)]
    struct IdClaims {
        sub: String,
        iss: String,
        aud: String,
        exp: u64,
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let id = IdClaims {
        sub: "user-1".into(),
        iss: issuer.as_str().to_owned(),
        aud: "a2a-client".into(),
        exp: now + 600,
    };
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("test-kid".into());
    let token = jsonwebtoken::encode(&header, &id, &enc).expect("encode");
    let claims = verifier
        .verify(&BearerToken::new(token), &AudienceAccount::new("nats-acct"))
        .await
        .expect("verify via trait");
    assert_eq!(claims.sub.as_str(), "user-1");
}

/// Signs `claims` as an RS256 token naming `test-kid`, the shape every guard
/// test below starts from before varying one thing about the JWK.
fn rs256_token_for(issuer: &OidcIssuerUrl, enc: &jsonwebtoken::EncodingKey) -> String {
    #[derive(Serialize)]
    struct IdClaims {
        sub: String,
        iss: String,
        aud: String,
        exp: u64,
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let id = IdClaims {
        sub: "user-guard".into(),
        iss: issuer.as_str().to_owned(),
        aud: "a2a-client".into(),
        exp: now + 600,
    };
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("test-kid".into());
    jsonwebtoken::encode(&header, &id, enc).expect("encode")
}

fn jwks_with_common(jwks: &JwkSet, mutate: impl FnOnce(&mut CommonParameters)) -> JwkSet {
    let mut jwk = jwks.keys[0].clone();
    mutate(&mut jwk.common);
    JwkSet { keys: vec![jwk] }
}

#[tokio::test]
async fn verify_rejects_algorithm_outside_the_allowlist() {
    let rng = &mut OsRng;
    let (jwks, _) = test_jwks_and_encoding_key(rng);
    let issuer = OidcIssuerUrl::parse("https://issuer.example").unwrap();
    let verifier = JwksOidcVerifier::with_static_jwks(issuer.clone(), vec!["a2a-client".into()], jwks);

    #[derive(Serialize)]
    struct IdClaims {
        sub: String,
        iss: String,
        aud: String,
        exp: u64,
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let id = IdClaims {
        sub: "user-hs".into(),
        iss: issuer.as_str().to_owned(),
        aud: "a2a-client".into(),
        exp: now + 600,
    };
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
    header.kid = Some("test-kid".into());
    let token =
        jsonwebtoken::encode(&header, &id, &jsonwebtoken::EncodingKey::from_secret(b"shared")).expect("encode hs256");

    let err = verifier
        .verify_internal(&BearerToken::new(token), &AudienceAccount::new("acct"))
        .await
        .unwrap_err();
    let AuthCalloutError::CredentialVerification(CredentialError::UnsupportedTokenAlgorithm { algorithm }) = err else {
        panic!("expected UnsupportedTokenAlgorithm, got {err:?}");
    };
    assert_eq!(algorithm, jsonwebtoken::Algorithm::HS256);
}

#[tokio::test]
async fn verify_rejects_a_jwk_published_for_encryption() {
    let rng = &mut OsRng;
    let (jwks, enc) = test_jwks_and_encoding_key(rng);
    let issuer = OidcIssuerUrl::parse("https://issuer.example").unwrap();
    let token = rs256_token_for(&issuer, &enc);
    let jwks = jwks_with_common(&jwks, |common| {
        common.public_key_use = Some(PublicKeyUse::Encryption);
        common.key_operations = None;
    });
    let verifier = JwksOidcVerifier::with_static_jwks(issuer, vec!["a2a-client".into()], jwks);

    let err = verifier
        .verify_internal(&BearerToken::new(token), &AudienceAccount::new("acct"))
        .await
        .unwrap_err();
    let AuthCalloutError::CredentialVerification(CredentialError::JwkNotPublishedForVerification { kid, algorithm }) =
        err
    else {
        panic!("expected JwkNotPublishedForVerification, got {err:?}");
    };
    assert_eq!(kid, "test-kid");
    assert_eq!(algorithm, jsonwebtoken::Algorithm::RS256);
}

#[tokio::test]
async fn verify_rejects_a_jwk_whose_key_ops_omit_verify() {
    let rng = &mut OsRng;
    let (jwks, enc) = test_jwks_and_encoding_key(rng);
    let issuer = OidcIssuerUrl::parse("https://issuer.example").unwrap();
    let token = rs256_token_for(&issuer, &enc);
    let jwks = jwks_with_common(&jwks, |common| {
        common.key_operations = Some(vec![KeyOperations::Encrypt]);
    });
    let verifier = JwksOidcVerifier::with_static_jwks(issuer, vec!["a2a-client".into()], jwks);

    let err = verifier
        .verify_internal(&BearerToken::new(token), &AudienceAccount::new("acct"))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::JwkNotPublishedForVerification { .. })
    ));
}

#[tokio::test]
async fn verify_rejects_a_jwk_pinned_to_a_different_rsa_algorithm() {
    let rng = &mut OsRng;
    let (jwks, enc) = test_jwks_and_encoding_key(rng);
    let issuer = OidcIssuerUrl::parse("https://issuer.example").unwrap();
    let token = rs256_token_for(&issuer, &enc);
    let jwks = jwks_with_common(&jwks, |common| {
        common.key_algorithm = Some(jsonwebtoken::jwk::KeyAlgorithm::PS512);
    });
    let verifier = JwksOidcVerifier::with_static_jwks(issuer, vec!["a2a-client".into()], jwks);

    let err = verifier
        .verify_internal(&BearerToken::new(token), &AudienceAccount::new("acct"))
        .await
        .unwrap_err();
    let AuthCalloutError::CredentialVerification(CredentialError::JwkNotPublishedForVerification { kid, algorithm }) =
        err
    else {
        panic!("expected JwkNotPublishedForVerification, got {err:?}");
    };
    assert_eq!(kid, "test-kid");
    assert_eq!(algorithm, jsonwebtoken::Algorithm::RS256);
}

#[tokio::test]
async fn verify_accepts_a_jwk_that_declares_the_asserted_algorithm() {
    let rng = &mut OsRng;
    let (jwks, enc) = test_jwks_and_encoding_key(rng);
    let issuer = OidcIssuerUrl::parse("https://issuer.example").unwrap();
    let token = rs256_token_for(&issuer, &enc);
    let jwks = jwks_with_common(&jwks, |common| {
        common.key_algorithm = Some(jsonwebtoken::jwk::KeyAlgorithm::RS256);
    });
    let verifier = JwksOidcVerifier::with_static_jwks(issuer, vec!["a2a-client".into()], jwks);

    let claims = verifier
        .verify_internal(&BearerToken::new(token), &AudienceAccount::new("acct"))
        .await
        .expect("declared alg matches the asserted one");
    assert_eq!(claims.sub.as_str(), "user-guard");
}

#[tokio::test]
async fn verify_accepts_a_jwk_that_declares_no_purpose_at_all() {
    let rng = &mut OsRng;
    let (jwks, enc) = test_jwks_and_encoding_key(rng);
    let issuer = OidcIssuerUrl::parse("https://issuer.example").unwrap();
    let token = rs256_token_for(&issuer, &enc);
    let jwks = jwks_with_common(&jwks, |common| {
        common.public_key_use = None;
        common.key_operations = None;
        common.key_algorithm = None;
    });
    let verifier = JwksOidcVerifier::with_static_jwks(issuer, vec!["a2a-client".into()], jwks);

    let claims = verifier
        .verify_internal(&BearerToken::new(token), &AudienceAccount::new("acct"))
        .await
        .expect("absent RFC 7517 members stay permissive");
    assert_eq!(claims.sub.as_str(), "user-guard");
}

#[test]
fn bearer_token_debug_does_not_leak_the_assertion() {
    let token = BearerToken::new("hhh.ppp.sss");
    let dbg = format!("{token:?}");
    assert!(!dbg.contains(token.as_str()), "{dbg}");
    // Each segment separately: a Debug that printed only the header, or only
    // the signature, would still disclose the assertion piecewise.
    for segment in ["hhh", "ppp", "sss"] {
        assert!(!dbg.contains(segment), "{dbg}");
    }
    assert!(dbg.contains("<redacted>"), "{dbg}");
    assert_eq!(token.as_str(), "hhh.ppp.sss");
}
