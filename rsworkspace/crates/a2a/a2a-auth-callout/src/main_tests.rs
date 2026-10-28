use a2a_auth_callout::AudienceAccount;
use a2a_auth_callout::credentials::mtls::ClientCertPem;
use a2a_auth_callout::error::AuthCalloutError;
use trogon_std::env::InMemoryEnv;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{
    build_mtls_verifier, build_oidc_verifier, env_required, load_nkey_public_env, load_nkey_seed_env, split_env_list,
};

#[derive(Debug, thiserror::Error)]
enum FixtureError {
    #[error(transparent)]
    Auth(#[from] AuthCalloutError),
    #[error(transparent)]
    Nkey(#[from] nkeys::error::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Certificate(#[from] rcgen::Error),
    #[error("configured test certificate verifier was disabled")]
    MissingVerifier,
}

#[test]
fn optional_lists_trim_entries_preserve_order_and_discard_blanks() {
    let env = InMemoryEnv::new();
    assert!(split_env_list(&env, "AUTH_CALLOUT_ALLOWED_ACCOUNTS").is_empty());
    env.set("AUTH_CALLOUT_ALLOWED_ACCOUNTS", " first, ,second,first,\n ");
    assert_eq!(
        split_env_list(&env, "AUTH_CALLOUT_ALLOWED_ACCOUNTS"),
        ["first", "second", "first"]
    );
    env.set("AUTH_CALLOUT_ALLOWED_ACCOUNTS", " , \n ");
    assert!(split_env_list(&env, "AUTH_CALLOUT_ALLOWED_ACCOUNTS").is_empty());
}

#[test]
fn required_variables_preserve_presence_and_report_the_missing_name() -> Result<(), AuthCalloutError> {
    let env = InMemoryEnv::new();
    assert!(matches!(
        env_required(&env, "REQUIRED"),
        Err(AuthCalloutError::MissingEnvVar("REQUIRED"))
    ));
    env.set("REQUIRED", "");
    assert_eq!(env_required(&env, "REQUIRED")?, "");
    env.set("REQUIRED", " value ");
    assert_eq!(env_required(&env, "REQUIRED")?, " value ");
    Ok(())
}

#[test]
fn nkey_environment_loaders_preserve_valid_material_and_reject_bad_configuration() -> Result<(), FixtureError> {
    let env = InMemoryEnv::new();
    assert!(matches!(
        load_nkey_seed_env(&env, "SEED"),
        Err(AuthCalloutError::MissingEnvVar("SEED"))
    ));
    assert!(matches!(
        load_nkey_public_env(&env, "PUBLIC"),
        Err(AuthCalloutError::MissingEnvVar("PUBLIC"))
    ));
    let key = nkeys::KeyPair::new_server();
    let seed = key.seed()?;
    env.set("SEED", format!(" {seed}\n"));
    env.set("PUBLIC", format!(" {}\n", key.public_key()));
    assert_eq!(
        load_nkey_seed_env(&env, "SEED")?.to_signing_keypair()?.public_key(),
        key.public_key()
    );
    assert_eq!(load_nkey_public_env(&env, "PUBLIC")?.as_str(), key.public_key());
    env.set("SEED", " \n ");
    assert!(matches!(
        load_nkey_seed_env(&env, "SEED"),
        Err(AuthCalloutError::WireFormat(_))
    ));
    env.set("PUBLIC", "invalid-key");
    assert!(matches!(
        load_nkey_public_env(&env, "PUBLIC"),
        Err(AuthCalloutError::WireFormat(_))
    ));
    Ok(())
}

#[tokio::test]
async fn oidc_remains_disabled_without_an_issuer_or_audience() {
    let env = InMemoryEnv::new();
    assert!(build_oidc_verifier(&env).await.is_none());
    env.set("AUTH_CALLOUT_OIDC_ISSUER", "///");
    assert!(build_oidc_verifier(&env).await.is_none());
    let server = MockServer::start().await;
    env.set("AUTH_CALLOUT_OIDC_ISSUER", server.uri());
    env.set("AUTH_CALLOUT_OIDC_AUDIENCES", " , ");
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    assert!(build_oidc_verifier(&env).await.is_none());
}

#[tokio::test]
async fn oidc_discovery_enables_a_matching_local_issuer() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "issuer": server.uri(), "jwks_uri": format!("{}/jwks", server.uri())
        })))
        .expect(1)
        .mount(&server)
        .await;
    let env = InMemoryEnv::new();
    env.set("AUTH_CALLOUT_OIDC_ISSUER", server.uri());
    env.set("AUTH_CALLOUT_OIDC_AUDIENCES", " client-a, client-b ");
    assert!(build_oidc_verifier(&env).await.is_some());
}

#[tokio::test]
async fn oidc_discovery_failure_disables_the_verifier() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
        .expect(1)
        .mount(&server)
        .await;
    let env = InMemoryEnv::new();
    env.set("AUTH_CALLOUT_OIDC_ISSUER", server.uri());
    env.set("AUTH_CALLOUT_OIDC_AUDIENCES", "client");
    assert!(build_oidc_verifier(&env).await.is_none());
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
    assert_eq!(claims.aud.as_str(), "fixture-account");
    Ok(())
}
