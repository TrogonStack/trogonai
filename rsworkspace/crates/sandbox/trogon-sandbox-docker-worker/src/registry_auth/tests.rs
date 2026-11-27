use std::time::{Duration, UNIX_EPOCH};

use bytes::Bytes;
use trogon_nats::AdvancedMockNatsClient;
use trogon_sandbox_contract::{
    CredentialError, CredentialIssueOutcome, CredentialIssueRequest, CredentialIssueResponse, ShortLivedToken, TokenId,
};
use trogon_std::env::InMemoryEnv;

use super::*;
use crate::support_tests::TestResult;

const ISSUE_SUBJECT: &str = "sandbox.credentials.issue.registry.acme";

fn client(
    mock: &AdvancedMockNatsClient,
) -> Result<CredentialClient<AdvancedMockNatsClient>, Box<dyn std::error::Error>> {
    Ok(CredentialClient::new(
        mock.clone(),
        CredentialProvider::Registry,
        TenantId::new("acme")?,
    ))
}

fn reply(value: &impl serde::Serialize) -> Result<Bytes, serde_json::Error> {
    Ok(Bytes::from(serde_json::to_vec(value)?))
}

fn reference() -> OciImageRef {
    OciImageRef::parse_unqualified("ghcr.io/acme/app:1.0")
}

fn ttl() -> Result<NonZeroU32, Box<dyn std::error::Error>> {
    Ok(NonZeroU32::new(60).ok_or("zero ttl")?)
}

fn issued(token: &str) -> CredentialIssueOutcome {
    CredentialIssueOutcome::Ok(CredentialIssueResponse {
        token_id: TokenId::new("tok-1"),
        token: ShortLivedToken::new(token),
        expires_at: UNIX_EPOCH + Duration::from_secs(100),
    })
}

fn read_config(dir: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_str(&std::fs::read_to_string(
        dir.join("config.json"),
    )?)?)
}

fn encoded(credentials: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(credentials.as_bytes())
}

#[test]
fn registry_keys_default_to_docker_hub_and_gain_a_scheme() {
    assert_eq!(normalize_registry_key(""), "https://index.docker.io/v1/");
    assert_eq!(normalize_registry_key("ghcr.io"), "https://ghcr.io");
    assert_eq!(normalize_registry_key("https://ghcr.io"), "https://ghcr.io");
    assert_eq!(normalize_registry_key("http://localhost:5000"), "http://localhost:5000");
}

#[test]
fn the_repository_glob_covers_every_tag_of_the_repository() {
    assert_eq!(repository_glob(&reference()), "ghcr.io/acme/app*");
    assert_eq!(
        repository_glob(&OciImageRef::parse_unqualified("library/alpine:3")),
        "library/alpine*"
    );
}

#[test]
fn the_config_directory_prefers_docker_config_then_home_then_tmp() {
    let env = InMemoryEnv::new();
    assert_eq!(docker_config_dir(&env), PathBuf::from("/tmp/.docker"));
    env.set("HOME", "/home/worker");
    assert_eq!(docker_config_dir(&env), PathBuf::from("/home/worker/.docker"));
    env.set("DOCKER_CONFIG", "/etc/docker-auth");
    assert_eq!(docker_config_dir(&env), PathBuf::from("/etc/docker-auth"));
}

#[test]
fn writing_auth_keeps_other_registries_and_replaces_the_same_one() -> TestResult {
    let dir = tempfile::tempdir()?;
    std::fs::write(
        dir.path().join("config.json"),
        r#"{"auths":{"https://other.io":{"auth":"b3RoZXI="}},"credsStore":"keep"}"#,
    )?;
    let token = ShortLivedToken::new("user:secret");
    write_docker_config_auth(dir.path(), "ghcr.io", &RegistryAuth::from_token(&token))?;
    let replaced = ShortLivedToken::new("user:rotated");
    write_docker_config_auth(dir.path(), "ghcr.io", &RegistryAuth::from_token(&replaced))?;

    let config = read_config(dir.path())?;
    assert_eq!(config["credsStore"], "keep");
    assert_eq!(config["auths"]["https://other.io"]["auth"], "b3RoZXI=");
    assert_eq!(config["auths"]["https://ghcr.io"]["auth"], encoded("user:rotated"));
    Ok(())
}

#[test]
fn writing_auth_creates_the_directory_and_recovers_from_an_unreadable_config() -> TestResult {
    let dir = tempfile::tempdir()?;
    let nested = dir.path().join("a").join("b");
    write_docker_config_auth(&nested, "", &RegistryAuth::from_token(&ShortLivedToken::new("tok")))?;
    let config = read_config(&nested)?;
    assert_eq!(
        config["auths"]["https://index.docker.io/v1/"]["auth"],
        encoded("token:tok")
    );

    std::fs::write(nested.join("config.json"), "not json")?;
    write_docker_config_auth(
        &nested,
        "ghcr.io",
        &RegistryAuth::from_token(&ShortLivedToken::new("u:p")),
    )?;
    assert_eq!(
        read_config(&nested)?["auths"]["https://ghcr.io"]["auth"],
        encoded("u:p")
    );
    Ok(())
}

#[test]
fn an_auths_entry_that_is_not_an_object_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("config.json"), r#"{"auths":[]}"#)?;
    let auth = RegistryAuth::from_token(&ShortLivedToken::new("u:p"));
    assert!(write_docker_config_auth(dir.path(), "ghcr.io", &auth).is_err());
    Ok(())
}

#[tokio::test]
async fn a_login_requests_a_pull_scoped_token_and_writes_it_to_the_config() -> TestResult {
    let mock = AdvancedMockNatsClient::new();
    mock.set_response(ISSUE_SUBJECT, reply(&issued("robot:secret"))?);
    let dir = tempfile::tempdir()?;

    login_with_client(
        &client(&mock)?,
        &reference(),
        &PullSecretId::new("pull-1"),
        ttl()?,
        dir.path(),
    )
    .await?;

    let payload = mock.requested_payloads().into_iter().next().ok_or("no request")?;
    let sent: CredentialIssueRequest = serde_json::from_slice(&payload)?;
    assert_eq!(sent.pull_secret_id, PullSecretId::new("pull-1"));
    assert_eq!(
        sent.scope,
        CredentialScope::RegistryPull {
            repository_glob: "ghcr.io/acme/app*".to_owned()
        }
    );
    assert_eq!(sent.ttl_seconds.get(), 60);
    assert_eq!(
        read_config(dir.path())?["auths"]["https://ghcr.io"]["auth"],
        encoded("robot:secret")
    );
    Ok(())
}

#[tokio::test]
async fn a_refused_issue_writes_nothing() -> TestResult {
    let mock = AdvancedMockNatsClient::new();
    let refusal = CredentialIssueOutcome::Err(CredentialError::ScopeDenied {
        message: "no".to_owned(),
    });
    mock.set_response(ISSUE_SUBJECT, reply(&refusal)?);
    let dir = tempfile::tempdir()?;

    let result = login_with_client(
        &client(&mock)?,
        &reference(),
        &PullSecretId::new("pull-1"),
        ttl()?,
        dir.path(),
    )
    .await;

    assert!(result.is_err());
    assert!(!dir.path().join("config.json").exists());
    Ok(())
}

#[tokio::test]
async fn images_without_a_pull_secret_need_no_login() -> TestResult {
    let image = SandboxImage::Registry {
        reference: reference(),
        pull_secret: None,
    };
    login_for_pull_secret(
        "nats://unused.invalid:4222",
        &TenantId::new("acme")?,
        &image,
        60,
        &InMemoryEnv::new(),
    )
    .await?;
    Ok(())
}
