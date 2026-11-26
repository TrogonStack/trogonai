use std::collections::BTreeMap;

use oci_client::secrets::RegistryAuth;
use secrecy::ExposeSecret;
use trogon_sandbox_contract::OciArtifactRef;

use crate::docker_credentials::DockerCredentials;
use crate::{OciClientConfig, OciError, RegistryCredentials, RegistryHost};

#[derive(Debug, Default)]
pub(crate) struct AuthResolver {
    docker: DockerCredentials,
    overrides: BTreeMap<RegistryHost, RegistryCredentials>,
}

impl AuthResolver {
    pub(crate) fn new(config: &OciClientConfig) -> Result<Self, OciError> {
        Ok(Self {
            docker: DockerCredentials::load(&config.docker_config)?,
            overrides: config.credentials.clone(),
        })
    }

    pub(crate) fn auth_for(&self, artifact: &OciArtifactRef) -> RegistryAuth {
        self.auth_for_registry(&artifact.registry)
    }

    pub(crate) fn auth_for_registry(&self, registry: &str) -> RegistryAuth {
        let normalized = normalize_registry(registry);
        self.overrides
            .get(normalized)
            .or_else(|| self.overrides.get(registry))
            .or_else(|| self.docker.lookup(normalized))
            .map_or(RegistryAuth::Anonymous, |credentials| {
                RegistryAuth::Basic(
                    credentials.username.clone(),
                    credentials.password.expose_secret().to_owned(),
                )
            })
    }
}

fn normalize_registry(registry: &str) -> &str {
    registry
        .trim_end_matches('/')
        .trim_start_matches("https://")
        .trim_start_matches("http://")
}
