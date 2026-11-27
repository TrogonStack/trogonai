use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use crate::K8sResourceNamingError;

/// Which family of names and labels the Kubernetes provider stamps on the objects it creates and recognizes as its
/// own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum K8sResourceNaming {
    #[default]
    Sandbox,
    Legacy,
}

impl K8sResourceNaming {
    pub fn managed_by(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox-worker",
            Self::Legacy => "legacy-runner",
        }
    }

    pub fn job_label(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox.trogon.live/job",
            Self::Legacy => "legacy/job",
        }
    }

    pub fn repo_label(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox.trogon.live/repo",
            Self::Legacy => "legacy/repo",
        }
    }

    pub fn egress_label(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox.trogon.live/egress",
            Self::Legacy => "legacy/egress",
        }
    }

    pub fn agent_name(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox-agent",
            Self::Legacy => "legacy-agent",
        }
    }

    pub fn warm_pool_name(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox-warm-pool",
            Self::Legacy => "legacy-warm-pool",
        }
    }

    pub fn secrets_bucket(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox-secrets",
            Self::Legacy => "legacy-secrets",
        }
    }

    pub(crate) fn owns_warm_labels(self, labels: &BTreeMap<String, String>) -> bool {
        labels.get("app.kubernetes.io/managed-by").map(String::as_str) == Some(self.managed_by())
    }

    pub(crate) fn other(self) -> Self {
        match self {
            Self::Sandbox => Self::Legacy,
            Self::Legacy => Self::Sandbox,
        }
    }
}

impl FromStr for K8sResourceNaming {
    type Err = K8sResourceNamingError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "sandbox" => Ok(Self::Sandbox),
            "legacy" => Ok(Self::Legacy),
            other => Err(K8sResourceNamingError::Unknown(other.to_string())),
        }
    }
}

impl fmt::Display for K8sResourceNaming {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Sandbox => "sandbox",
            Self::Legacy => "legacy",
        })
    }
}

#[cfg(test)]
mod tests;
