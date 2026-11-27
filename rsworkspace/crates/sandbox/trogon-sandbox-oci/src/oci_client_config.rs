use std::collections::BTreeMap;

use crate::{DockerConfigSource, RegistryCredentials, RegistryHost};

/// Registry access settings for [`OciClient`](crate::OciClient).
#[derive(Debug, Clone, Default)]
pub struct OciClientConfig {
    /// Registries reached over plain HTTP instead of HTTPS.
    pub insecure_registries: Vec<RegistryHost>,
    /// Credentials that take precedence over stored Docker credentials.
    pub credentials: BTreeMap<RegistryHost, RegistryCredentials>,
    pub docker_config: DockerConfigSource,
}
