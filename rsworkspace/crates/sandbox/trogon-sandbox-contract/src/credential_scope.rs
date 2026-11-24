use serde::{Deserialize, Serialize};

use crate::LeaseId;

/// What an issued credential may do, tagged by `kind`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialScope {
    RegistryPull { repository_glob: String },
    RegistryPush { repository_glob: String },
    S3ReadOnly { bucket: String, prefix: String },
    S3ReadWrite { bucket: String, prefix: String },
    DaytonaApi { workspace_id: String },
    AzureServicePrincipal { resource_id: String },
    SshSession { lease_id: LeaseId },
}
