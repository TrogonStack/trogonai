use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use base64::Engine;
use secrecy::SecretString;
use serde::Deserialize;

use crate::{DockerConfigSource, OciError, RegistryCredentials};

#[derive(Deserialize)]
struct DockerConfigFile {
    auths: Option<BTreeMap<String, DockerAuthEntry>>,
}

#[derive(Deserialize)]
struct DockerAuthEntry {
    auth: Option<String>,
    username: Option<String>,
    password: Option<String>,
}

impl DockerAuthEntry {
    /// The encoded `auth` pair wins over separate `username` and `password` fields.
    fn into_credentials(self) -> Option<RegistryCredentials> {
        if let Some((username, password)) = self.auth.as_deref().and_then(decode_basic_auth) {
            return Some(RegistryCredentials::new(username, SecretString::from(password)));
        }
        match (self.username, self.password) {
            (Some(username), Some(password)) => Some(RegistryCredentials::new(username, SecretString::from(password))),
            _ => None,
        }
    }
}

/// Credentials stored by `docker login`, keyed by the registry key of the config file.
#[derive(Debug, Default)]
pub(crate) struct DockerCredentials {
    by_registry: BTreeMap<String, RegistryCredentials>,
}

impl DockerCredentials {
    pub(crate) fn load(source: &DockerConfigSource) -> Result<Self, OciError> {
        let path = match source {
            DockerConfigSource::Disabled => return Ok(Self::default()),
            DockerConfigSource::Directory(directory) => directory.join("config.json"),
            DockerConfigSource::Environment => config_path(std::env::var_os("DOCKER_CONFIG"), std::env::var_os("HOME")),
        };
        Self::load_file(&path)
    }

    fn load_file(path: &Path) -> Result<Self, OciError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)?;
        let parsed: DockerConfigFile = serde_json::from_str(&raw)
            .map_err(|error| OciError::Config(format!("invalid docker config.json: {error}")))?;
        let by_registry = parsed
            .auths
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(registry, entry)| Some((registry, entry.into_credentials()?)))
            .collect();
        Ok(Self { by_registry })
    }

    /// Looks up a registry by its bare name, its `https://` URL, or its legacy `/v1/` URL.
    pub(crate) fn lookup(&self, registry: &str) -> Option<&RegistryCredentials> {
        self.by_registry
            .get(registry)
            .or_else(|| self.by_registry.get(&format!("https://{registry}/")))
            .or_else(|| self.by_registry.get(&format!("https://{registry}/v1/")))
    }
}

fn config_path(docker_config: Option<OsString>, home: Option<OsString>) -> PathBuf {
    match docker_config {
        Some(directory) => PathBuf::from(directory).join("config.json"),
        None => home
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".docker")
            .join("config.json"),
    }
}

fn decode_basic_auth(encoded: &str) -> Option<(String, String)> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).ok()?;
    let pair = String::from_utf8(bytes).ok()?;
    let (username, password) = pair.split_once(':')?;
    Some((username.to_string(), password.to_string()))
}

#[cfg(test)]
mod tests;
