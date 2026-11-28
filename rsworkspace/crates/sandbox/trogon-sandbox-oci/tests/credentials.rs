mod support;

use std::collections::BTreeMap;

use base64::Engine;
use secrecy::SecretString;
use support::{TestResult, push_artifact};
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use trogon_sandbox_oci::{DockerConfigSource, OciClient, OciClientConfig, OciError, RegistryCredentials, RegistryHost};

/// A server that answers every request with 202 and reports the Authorization header it saw.
async fn probe() -> Result<(String, tokio::task::JoinHandle<Option<String>>), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?.to_string();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.ok()?;
        let mut buffer = vec![0u8; 8192];
        let read = socket.read(&mut buffer).await.ok()?;
        let request = String::from_utf8_lossy(&buffer[..read]).to_string();
        let header = request
            .lines()
            .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
            .map(|line| {
                line.split_once(':')
                    .map(|(_, value)| value.trim().to_string())
                    .unwrap_or_default()
            });
        tokio::io::AsyncWriteExt::write_all(
            &mut socket,
            b"HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
        )
        .await
        .ok()?;
        header
    });
    Ok((address, task))
}

fn basic(user: &str, password: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))
    )
}

async fn authorization_for(
    config_json: Option<&str>,
    key: &str,
    overrides: bool,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let (address, task) = probe().await?;
    let directory = tempfile::tempdir()?;
    if let Some(json) = config_json {
        std::fs::write(
            directory.path().join("config.json"),
            json.replace("{key}", key).replace("{host}", &address),
        )?;
    }
    let mut config = OciClientConfig {
        insecure_registries: vec![RegistryHost::new(&address)?],
        docker_config: DockerConfigSource::Directory(directory.path().to_path_buf()),
        credentials: BTreeMap::new(),
    };
    if overrides {
        config.credentials.insert(
            RegistryHost::new(&address)?,
            RegistryCredentials::new("override", SecretString::from("wins")),
        );
    }
    let client = OciClient::try_new(config)?;
    let (artifact, _) = support::artifact(&address, b"x")?;
    client.delete_artifact(&artifact).await?;
    Ok(task.await?)
}

#[tokio::test]
async fn stored_docker_credentials_are_used_for_matching_registries() -> TestResult {
    let encoded = base64::engine::general_purpose::STANDARD.encode("alice:s3cret");
    let with_auth = format!(r#"{{"auths": {{"{{key}}": {{"auth": "{encoded}"}}}}}}"#);
    let with_fields = r#"{"auths": {"{key}": {"username": "bob", "password": "hunter2"}}}"#;
    let invalid = r#"{"auths": {"{key}": {"auth": "!!!not-base64"}}}"#;
    let half = r#"{"auths": {"{key}": {"username": "bob"}}}"#;
    let no_colon = format!(
        r#"{{"auths": {{"{{key}}": {{"auth": "{}"}}}}}}"#,
        base64::engine::general_purpose::STANDARD.encode("nocolon")
    );
    let not_utf8 = format!(
        r#"{{"auths": {{"{{key}}": {{"auth": "{}"}}}}}}"#,
        base64::engine::general_purpose::STANDARD.encode([0xff, 0xfe, b':'])
    );

    assert_eq!(
        authorization_for(Some(&with_auth), "{host}", false).await?,
        Some(basic("alice", "s3cret"))
    );
    assert_eq!(
        authorization_for(Some(with_fields), "{host}", false).await?,
        Some(basic("bob", "hunter2"))
    );
    assert_eq!(
        authorization_for(Some(with_fields), "https://{host}/", false).await?,
        Some(basic("bob", "hunter2"))
    );
    assert_eq!(
        authorization_for(Some(with_fields), "https://{host}/v1/", false).await?,
        Some(basic("bob", "hunter2"))
    );
    assert_eq!(
        authorization_for(Some(with_fields), "other.invalid", false).await?,
        None
    );
    for unusable in [half, "{}"] {
        assert_eq!(authorization_for(Some(unusable), "{host}", false).await?, None);
    }
    for malformed in [invalid, no_colon.as_str(), not_utf8.as_str()] {
        assert!(authorization_for(Some(malformed), "{host}", false).await.is_err());
    }
    assert_eq!(authorization_for(None, "{host}", false).await?, None);
    Ok(())
}

#[tokio::test]
async fn configured_credentials_win_over_stored_ones() -> TestResult {
    let stored = r#"{"auths": {"{key}": {"username": "bob", "password": "hunter2"}}}"#;
    assert_eq!(
        authorization_for(Some(stored), "{host}", true).await?,
        Some(basic("override", "wins"))
    );
    Ok(())
}

#[tokio::test]
async fn unreadable_docker_configs_fail_strictly_and_fall_back_leniently() -> TestResult {
    let directory = tempfile::tempdir()?;
    std::fs::write(directory.path().join("config.json"), "not json")?;
    let config = || OciClientConfig {
        docker_config: DockerConfigSource::Directory(directory.path().to_path_buf()),
        ..Default::default()
    };
    assert!(matches!(OciClient::try_new(config()).err(), Some(OciError::Config(_))));

    let lenient = OciClient::new(config());
    let registry = support::FakeRegistry::start().await?;
    let artifact = push_artifact(&registry, "repo", trogon_sandbox_contract::ArtifactType::SbomV1, b"x").await?;
    // The lenient client still builds; it simply has no stored credentials.
    assert!(lenient.delete_artifact(&artifact).await.is_err());

    let unreadable = tempfile::tempdir()?;
    std::fs::create_dir(unreadable.path().join("config.json"))?;
    let source = DockerConfigSource::Directory(unreadable.path().to_path_buf());
    let error = OciClient::try_new(OciClientConfig {
        docker_config: source,
        ..Default::default()
    })
    .err();
    assert!(matches!(error, Some(OciError::Io(_))), "{error:?}");
    Ok(())
}

#[tokio::test]
async fn a_credential_store_without_an_inline_entry_fails_the_request_instead_of_going_anonymous() -> TestResult {
    let directory = tempfile::tempdir()?;
    std::fs::write(directory.path().join("config.json"), r#"{"credsStore":"desktop"}"#)?;
    let registry = support::FakeRegistry::start().await?;
    let artifact = push_artifact(&registry, "repo", trogon_sandbox_contract::ArtifactType::SbomV1, b"x").await?;
    let client = OciClient::try_new(OciClientConfig {
        docker_config: DockerConfigSource::Directory(directory.path().to_path_buf()),
        ..Default::default()
    })?;
    let error = client.delete_artifact(&artifact).await.err();
    assert!(matches!(error, Some(OciError::Config(_))), "{error:?}");
    Ok(())
}
