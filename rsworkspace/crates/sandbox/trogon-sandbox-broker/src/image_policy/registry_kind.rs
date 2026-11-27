use serde::{Deserialize, Serialize};

/// Identifies a registry so a `RegistryProfile` can grant access to everything
/// hosted there without enumerating every repository as a separate pattern.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RegistryKind {
    DockerHub,
    Ghcr,
    Ecr { region: String },
    SelfHosted { host: String },
}

impl RegistryKind {
    pub(crate) fn matches_registry(&self, registry: &str) -> bool {
        match self {
            RegistryKind::DockerHub => {
                matches!(registry, "" | "docker.io" | "registry-1.docker.io" | "index.docker.io")
            }
            RegistryKind::Ghcr => registry == "ghcr.io",
            RegistryKind::Ecr { region } => registry.ends_with(&format!(".dkr.ecr.{region}.amazonaws.com")),
            RegistryKind::SelfHosted { host } => registry == host,
        }
    }
}
