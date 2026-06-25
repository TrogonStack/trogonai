use super::*;
use crate::jwt::SigningKey;
use crate::signing_key_source::{KeyVersion, SigningKeyHandle};
use std::time::Duration;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::jwk::{
    AlgorithmParameters, CommonParameters, Jwk, KeyOperations, PublicKeyUse, RSAKeyParameters, RSAKeyType,
};
use rand::rngs::OsRng;
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
            key_operations: Some(vec![KeyOperations::Sign]),
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
    use jsonwebtoken::jwk::{
        AlgorithmParameters, CommonParameters, EllipticCurveKeyParameters, EllipticCurveKeyType, Jwk,
    };
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

    // Craft a fake JWT whose kid matches the EC JWK; decode_header will succeed
    // but decoding_key_for_jwk must reject the non-RSA key.
    // We can't sign with the EC key easily, but we can make a header-only token
    // that references the EC kid.  decode_header just parses the header.
    let header_b64 =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"alg":"ES256","kid":"ec-kid","typ":"JWT"}"#);
    let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{}");
    let fake_token = format!("{header_b64}.{payload_b64}.sig");

    let err = verifier
        .verify_internal(&BearerToken::new(fake_token), &AudienceAccount::new("acct"))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(_))
    ));
}

#[tokio::test]
async fn oidc_verifier_trait_delegates_to_verify_internal() {
    // Exercise the OidcVerifier::verify blanket impl on JwksOidcVerifier.
    use rand::rngs::OsRng;
    let rng = &mut OsRng;
    let (jwks, enc) = test_jwks_and_encoding_key(rng);
    let issuer = OidcIssuerUrl::parse("https://issuer.example").unwrap();
    let verifier: &dyn OidcVerifier =
        &JwksOidcVerifier::with_static_jwks(issuer.clone(), vec!["a2a-client".into()], jwks);
    use serde::Serialize;
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
