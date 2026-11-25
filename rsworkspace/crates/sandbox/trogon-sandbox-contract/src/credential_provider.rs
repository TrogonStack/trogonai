use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::CredentialProviderError;

/// The system a credential is issued for.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialProvider {
    Registry,
    S3,
    Daytona,
    Modal,
    Azure,
    Docker,
    /// Session tokens for SSH access to a sandbox.
    Ssh,
}

impl CredentialProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Registry => "registry",
            Self::S3 => "s3",
            Self::Daytona => "daytona",
            Self::Modal => "modal",
            Self::Azure => "azure",
            Self::Docker => "docker",
            Self::Ssh => "ssh",
        }
    }
}

impl FromStr for CredentialProvider {
    type Err = CredentialProviderError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "registry" => Ok(Self::Registry),
            "s3" => Ok(Self::S3),
            "daytona" => Ok(Self::Daytona),
            "modal" => Ok(Self::Modal),
            "azure" => Ok(Self::Azure),
            "docker" => Ok(Self::Docker),
            "ssh" => Ok(Self::Ssh),
            _ => Err(CredentialProviderError::Unknown),
        }
    }
}

impl fmt::Display for CredentialProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
