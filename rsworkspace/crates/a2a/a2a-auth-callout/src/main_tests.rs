use a2a_auth_callout::AudienceAccount;
use a2a_auth_callout::credentials::mtls::ClientCertPem;
use a2a_auth_callout::credentials::oidc::{BearerToken, JwksOidcVerifier};
use a2a_auth_callout::error::AuthCalloutError;
use a2a_auth_callout::tenant_admission::{ENV_TENANT_MEMBERSHIPS, TenantAdmissionError};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rsa::RsaPrivateKey;
use rsa::pkcs8::EncodePrivateKey;
use rsa::traits::PublicKeyParts;
use trogon_nats::{ClientCertificate, Location, NatsTls};
use trogon_std::env::InMemoryEnv;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use a2a_nats::{A2aPrefixError, ENV_A2A_PREFIX};

use super::{
    build_mtls_verifier, build_oidc_verifier, build_oidc_verifier_with_discovery, build_tenant_admission, nats_config,
    resolve_a2a_prefix, split_env_list,
};

#[derive(Debug, thiserror::Error)]
enum FixtureError {
    #[error(transparent)]
    Auth(#[from] AuthCalloutError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Certificate(#[from] rcgen::Error),
    #[error(transparent)]
    Rsa(#[from] rsa::errors::Error),
    #[error(transparent)]
    PrivateKey(#[from] rsa::pkcs8::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Jwt(#[from] jsonwebtoken::errors::Error),
    #[error("configured test verifier was disabled")]
    MissingVerifier,
    #[error("configured NATS servers were discarded")]
    MissingNatsConfig,
}

#[tokio::test]
async fn tenant_admission_is_required_and_validated_before_use() -> Result<(), FixtureError> {
    let env = InMemoryEnv::new();
    assert!(matches!(
        build_tenant_admission(&env).await,
        Err(AuthCalloutError::MissingEnvVar(ENV_TENANT_MEMBERSHIPS))
    ));
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("memberships.json");
    env.set(ENV_TENANT_MEMBERSHIPS, path.to_string_lossy().into_owned());
    assert!(matches!(
        build_tenant_admission(&env).await,
        Err(AuthCalloutError::TenantAdmission(TenantAdmissionError::Read(_)))
    ));
    std::fs::write(&path, b"invalid")?;
    assert!(matches!(
        build_tenant_admission(&env).await,
        Err(AuthCalloutError::TenantAdmission(TenantAdmissionError::Json(_)))
    ));
    std::fs::write(&path, br#"{"version":1,"memberships":[]}"#)?;
    assert!(build_tenant_admission(&env).await.is_ok());
    Ok(())
}

#[test]
fn optional_lists_trim_entries_preserve_order_and_discard_blanks() {
    let env = InMemoryEnv::new();
    assert!(split_env_list(&env, "AUTH_CALLOUT_OIDC_AUDIENCES").is_empty());
    env.set("AUTH_CALLOUT_OIDC_AUDIENCES", " first, ,second,first,\n ");
    assert_eq!(
        split_env_list(&env, "AUTH_CALLOUT_OIDC_AUDIENCES"),
        ["first", "second", "first"]
    );
    env.set("AUTH_CALLOUT_OIDC_AUDIENCES", " , \n ");
    assert!(split_env_list(&env, "AUTH_CALLOUT_OIDC_AUDIENCES").is_empty());
}

#[test]
fn the_grant_prefix_is_the_default_namespace_unless_a_valid_one_is_configured() -> Result<(), A2aPrefixError> {
    let env = InMemoryEnv::new();
    assert_eq!(resolve_a2a_prefix(&env)?.as_str(), "a2a");
    env.set(ENV_A2A_PREFIX, "acme.a2a");
    assert_eq!(resolve_a2a_prefix(&env)?.as_str(), "acme.a2a");
    env.set(ENV_A2A_PREFIX, "invalid.*");
    assert!(resolve_a2a_prefix(&env).is_err());
    Ok(())
}

#[test]
fn nats_settings_keep_every_server_and_the_shared_client_certificate() -> Result<(), FixtureError> {
    let env = InMemoryEnv::new();
    assert_eq!(
        nats_config(&env).map(|config| config.servers),
        Some(vec!["localhost:4222".to_owned()])
    );
    env.set("NATS_URL", " tls://one:4443 , tls://two:4443 ");
    env.set("NATS_CERT", "certs/callout.crt");
    env.set("NATS_KEY", "file://certs/callout.key");
    let config = nats_config(&env).ok_or(FixtureError::MissingNatsConfig)?;
    assert_eq!(config.servers, ["tls://one:4443", "tls://two:4443"]);
    assert_eq!(
        config.tls,
        Ok(NatsTls::Enabled {
            ca: None,
            client_cert: Some(ClientCertificate {
                cert: Location::Path("certs/callout.crt".into()),
                key: Location::Path("certs/callout.key".into()),
            }),
        })
    );
    env.set("NATS_URL", " , ");
    assert!(nats_config(&env).is_none());
    Ok(())
}

#[tokio::test]
async fn oidc_remains_disabled_without_an_issuer_or_audience() {
    let env = InMemoryEnv::new();
    assert!(build_oidc_verifier(&env).await.is_none());
    env.set("AUTH_CALLOUT_OIDC_ISSUER", "///");
    assert!(build_oidc_verifier(&env).await.is_none());
    let server = MockServer::start().await;
    env.set(
        "AUTH_CALLOUT_OIDC_ISSUER",
        server.uri().replacen("http://", "https://", 1),
    );
    env.set("AUTH_CALLOUT_OIDC_AUDIENCES", " , ");
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    assert!(build_oidc_verifier(&env).await.is_none());
}

#[tokio::test]
async fn oidc_discovery_refuses_a_plaintext_issuer_before_network_io() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "issuer": server.uri(), "jwks_uri": format!("{}/jwks", server.uri())
        })))
        .expect(0)
        .mount(&server)
        .await;
    let env = InMemoryEnv::new();
    env.set("AUTH_CALLOUT_OIDC_ISSUER", server.uri());
    env.set("AUTH_CALLOUT_OIDC_AUDIENCES", " client-a, client-b ");
    assert!(build_oidc_verifier(&env).await.is_none());
}

#[tokio::test]
async fn oidc_discovery_failure_disables_the_verifier() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
        .expect(0)
        .mount(&server)
        .await;
    let env = InMemoryEnv::new();
    env.set(
        "AUTH_CALLOUT_OIDC_ISSUER",
        server.uri().replacen("http://", "https://", 1),
    );
    env.set("AUTH_CALLOUT_OIDC_AUDIENCES", "client");
    assert!(build_oidc_verifier(&env).await.is_none());
}

#[tokio::test]
async fn successful_oidc_discovery_wires_the_exact_issuer_and_parsed_audiences() -> Result<(), FixtureError> {
    let key = RsaPrivateKey::new(&mut rand_core::OsRng, 2048)?;
    let public = key.to_public_key();
    let jwks = serde_json::from_value(serde_json::json!({"keys": [{
        "kty": "RSA", "kid": "startup", "use": "sig", "alg": "RS256",
        "n": URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),
        "e": URL_SAFE_NO_PAD.encode(public.e().to_bytes_be()),
    }]}))?;
    let encoding = jsonwebtoken::EncodingKey::from_rsa_pem(key.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)?.as_bytes())?;
    let env = InMemoryEnv::new();
    env.set("AUTH_CALLOUT_OIDC_ISSUER", "https://issuer.example/tenant/");
    env.set("AUTH_CALLOUT_OIDC_AUDIENCES", " client-a, ,client-b ");
    let verifier = build_oidc_verifier_with_discovery(&env, |issuer, audiences| async move {
        assert_eq!(issuer.as_str(), "https://issuer.example/tenant/");
        assert_eq!(audiences, ["client-a", "client-b"]);
        Ok(JwksOidcVerifier::with_static_jwks(issuer, audiences, jwks))
    })
    .await
    .ok_or(FixtureError::MissingVerifier)?;
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("startup".into());
    let mut claims = serde_json::json!({
        "iss": "https://issuer.example/tenant/", "aud": "client-b", "sub": "startup-caller",
        "exp": time::OffsetDateTime::now_utc().unix_timestamp() + 600,
    });
    let account = AudienceAccount::new("tenant");
    let token = jsonwebtoken::encode(&header, &claims, &encoding)?;
    let verified = verifier.verify(&BearerToken::new(token), &account).await?;
    assert_eq!(verified.claims().sub.as_str(), "startup-caller");
    claims["aud"] = serde_json::json!("other-client");
    let token = jsonwebtoken::encode(&header, &claims, &encoding)?;
    assert!(verifier.verify(&BearerToken::new(token), &account).await.is_err());
    claims["aud"] = serde_json::json!("client-a");
    claims["iss"] = serde_json::json!("https://other-issuer.example/tenant/");
    let token = jsonwebtoken::encode(&header, &claims, &encoding)?;
    assert!(verifier.verify(&BearerToken::new(token), &account).await.is_err());
    Ok(())
}

#[test]
fn mtls_requires_a_readable_utf8_trust_bundle() -> Result<(), std::io::Error> {
    let env = InMemoryEnv::new();
    assert!(build_mtls_verifier(&env).is_none());
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("anchors.pem");
    env.set("AUTH_CALLOUT_MTLS_TRUST_ANCHORS", path.to_string_lossy().into_owned());
    assert!(build_mtls_verifier(&env).is_none());
    std::fs::write(&path, b"\xff")?;
    assert!(build_mtls_verifier(&env).is_none());
    Ok(())
}

#[tokio::test]
async fn mtls_factory_loads_the_configured_anchor_for_verification() -> Result<(), FixtureError> {
    let key = rcgen::KeyPair::generate()?;
    let mut params = rcgen::CertificateParams::default();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "fixture-client");
    let certificate = params.self_signed(&key)?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("anchors.pem");
    std::fs::write(&path, certificate.pem())?;
    let env = InMemoryEnv::new();
    env.set("AUTH_CALLOUT_MTLS_TRUST_ANCHORS", path.to_string_lossy().into_owned());
    let verifier = build_mtls_verifier(&env).ok_or(FixtureError::MissingVerifier)?;
    let claims = verifier
        .verify(
            &ClientCertPem::new(certificate.pem()),
            &AudienceAccount::new("fixture-account"),
        )
        .await?;
    assert_eq!(claims.claims().aud.as_str(), "fixture-account");
    Ok(())
}
