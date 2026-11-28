use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use base64::Engine;
use secrecy::SecretString;
use serde::Deserialize;
use trogon_std::env::ReadEnv;

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
    ///
    /// An `auth` value that is present but not a base64 `user:password` pair is refused, not
    /// skipped: skipping it would send the request anonymously under the wrong identity.
    fn into_credentials(self, registry: &str) -> Result<Option<RegistryCredentials>, OciError> {
        if let Some(encoded) = self.auth.as_deref().filter(|encoded| !encoded.is_empty()) {
            let (username, password) = decode_basic_auth(encoded).ok_or_else(|| {
                OciError::Config(format!(
                    "docker config.json auth for {registry} is not a base64 user:password pair"
                ))
            })?;
            return Ok(Some(RegistryCredentials::new(username, SecretString::from(password))));
        }
        Ok(match (self.username, self.password) {
            (Some(username), Some(password)) => Some(RegistryCredentials::new(username, SecretString::from(password))),
            _ => None,
        })
    }
}

/// Credentials stored by `docker login`, keyed by the registry key of the config file.
#[derive(Debug, Default)]
pub(crate) struct DockerCredentials {
    by_registry: BTreeMap<String, RegistryCredentials>,
}

impl DockerCredentials {
    pub(crate) fn load(source: &DockerConfigSource, env: &impl ReadEnv) -> Result<Self, OciError> {
        let path = match source {
            DockerConfigSource::Disabled => return Ok(Self::default()),
            DockerConfigSource::Directory(directory) => directory.join("config.json"),
            DockerConfigSource::Environment => config_path(env.var_os("DOCKER_CONFIG"), env.var_os("HOME"))?,
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
        let mut by_registry = BTreeMap::new();
        for (registry, entry) in parsed.auths.unwrap_or_default() {
            if let Some(credentials) = entry.into_credentials(&registry)? {
                by_registry.insert(registry, credentials);
            }
        }
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

/// Where the Docker config file lives. Without `DOCKER_CONFIG` or `HOME` there is no place to
/// look: the working directory is never searched, because credentials would then come from
/// whatever directory the process started in.
fn config_path(docker_config: Option<OsString>, home: Option<OsString>) -> Result<PathBuf, OciError> {
    match (docker_config, home) {
        (Some(directory), _) => Ok(PathBuf::from(directory).join("config.json")),
        (None, Some(home)) => Ok(PathBuf::from(home).join(".docker").join("config.json")),
        (None, None) => Err(OciError::Config(
            "cannot locate the docker config: neither DOCKER_CONFIG nor HOME is set".into(),
        )),
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
