use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use anyhow::Context;
use base64::Engine;
use serde_json::{Map, Value};
use trogon_nats::RequestClient;
use trogon_sandbox_contract::{CredentialProvider, CredentialScope, OciImageRef, PullSecretId, SandboxImage, TenantId};
use trogon_sandbox_credential_client::{CredentialClient, IssueParams, RegistryAuth};
use trogon_sandbox_protocol::nats_connect;
use trogon_std::env::ReadEnv;

mod constants;

use constants::DEFAULT_REGISTRY_KEY;

/// Writes the registry login a lease's pull secret resolves to into the Docker client configuration, so the
/// following pull is authenticated. Images without a pull secret need no login.
pub async fn login_for_pull_secret(
    nats_url: &str,
    tenant: &TenantId,
    image: &SandboxImage,
    ttl_seconds: u32,
    env: &impl ReadEnv,
) -> anyhow::Result<()> {
    let SandboxImage::Registry {
        reference,
        pull_secret: Some(pull_secret_id),
    } = image
    else {
        return Ok(());
    };
    let ttl_seconds = NonZeroU32::new(ttl_seconds).context("a registry login needs a non-zero ttl")?;
    let nats = nats_connect::connect(nats_url).await?;
    let client = CredentialClient::new(nats, CredentialProvider::Registry, tenant.clone());
    login_with_client(&client, reference, pull_secret_id, ttl_seconds, &docker_config_dir(env)).await
}

async fn login_with_client<C: RequestClient>(
    client: &CredentialClient<C>,
    reference: &OciImageRef,
    pull_secret_id: &PullSecretId,
    ttl_seconds: NonZeroU32,
    config_dir: &Path,
) -> anyhow::Result<()> {
    let issued = client
        .issue(IssueParams {
            pull_secret_id: pull_secret_id.clone(),
            scope: CredentialScope::RegistryPull {
                repository_glob: repository_glob(reference),
            },
            ttl_seconds,
            requester: None,
        })
        .await
        .map_err(|e| anyhow::anyhow!("credential issue failed: {e}"))?;
    let auth = RegistryAuth::from_token(&issued.token);
    write_docker_config_auth(config_dir, &reference.registry, &auth)
}

fn write_docker_config_auth(config_dir: &Path, registry: &str, auth: &RegistryAuth) -> anyhow::Result<()> {
    std::fs::create_dir_all(config_dir).context("create docker config dir")?;
    let config_path = config_dir.join("config.json");

    let mut root: Map<String, Value> = if config_path.is_file() {
        let raw = std::fs::read_to_string(&config_path).context("read docker config")?;
        serde_json::from_str(&raw).unwrap_or_default()
    } else {
        Map::new()
    };

    let auths = root
        .entry("auths")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .context("docker config auths is not an object")?;

    let credentials = format!("{}:{}", auth.username(), auth.expose_password());
    let mut entry = Map::new();
    entry.insert(
        "auth".to_owned(),
        Value::String(base64::engine::general_purpose::STANDARD.encode(credentials.as_bytes())),
    );
    auths.insert(normalize_registry_key(registry), Value::Object(entry));

    let serialized = serde_json::to_string_pretty(&root).context("serialize docker config")?;
    std::fs::write(&config_path, serialized).context("write docker config")?;
    Ok(())
}

fn docker_config_dir(env: &impl ReadEnv) -> PathBuf {
    match env.var("DOCKER_CONFIG") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => env
            .var("HOME")
            .map_or_else(|_| PathBuf::from("/tmp"), PathBuf::from)
            .join(".docker"),
    }
}

fn normalize_registry_key(registry: &str) -> String {
    if registry.is_empty() {
        DEFAULT_REGISTRY_KEY.to_string()
    } else if registry.starts_with("http://") || registry.starts_with("https://") {
        registry.to_string()
    } else {
        format!("https://{registry}")
    }
}

fn repository_glob(reference: &OciImageRef) -> String {
    if reference.registry.is_empty() {
        format!("{}*", reference.repository)
    } else {
        format!("{}/{}*", reference.registry, reference.repository)
    }
}

#[cfg(test)]
mod tests;
