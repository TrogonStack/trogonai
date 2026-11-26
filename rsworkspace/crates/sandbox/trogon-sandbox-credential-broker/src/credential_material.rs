use std::fmt;

use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{CredentialScope, ShortLivedToken};

use crate::CredentialMaterialError;

/// The secret a stored credential holds, tagged by `kind`.
///
/// `Debug` shows the kind and never the secret fields.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialMaterial {
    RegistryBasic {
        username: String,
        password: String,
    },
    RegistryToken {
        token: String,
    },
    S3Keys {
        access_key_id: String,
        secret_access_key: String,
        session_token: Option<String>,
    },
    DaytonaApi {
        api_token: String,
    },
    AzureServicePrincipal {
        client_id: String,
        client_secret: String,
        tenant_id: String,
    },
    SshGateway {
        gateway_id: String,
    },
}

impl CredentialMaterial {
    fn kind(&self) -> &'static str {
        match self {
            Self::RegistryBasic { .. } => "RegistryBasic",
            Self::RegistryToken { .. } => "RegistryToken",
            Self::S3Keys { .. } => "S3Keys",
            Self::DaytonaApi { .. } => "DaytonaApi",
            Self::AzureServicePrincipal { .. } => "AzureServicePrincipal",
            Self::SshGateway { .. } => "SshGateway",
        }
    }

    /// The token value this material yields for `scope`, or an error when the material is for another kind of
    /// scope.
    pub fn issue_for_scope(&self, scope: &CredentialScope) -> Result<ShortLivedToken, CredentialMaterialError> {
        let value = match (self, scope) {
            (
                Self::RegistryBasic { username, password },
                CredentialScope::RegistryPull { .. } | CredentialScope::RegistryPush { .. },
            ) => format!("{username}:{password}"),
            (
                Self::RegistryToken { token },
                CredentialScope::RegistryPull { .. } | CredentialScope::RegistryPush { .. },
            ) => token.clone(),
            (
                Self::S3Keys {
                    access_key_id,
                    secret_access_key,
                    session_token,
                },
                CredentialScope::S3ReadOnly { .. } | CredentialScope::S3ReadWrite { .. },
            ) => {
                let mut out = format!("{access_key_id}:{secret_access_key}");
                if let Some(session_token) = session_token {
                    out.push(':');
                    out.push_str(session_token);
                }
                out
            }
            (Self::DaytonaApi { api_token }, CredentialScope::DaytonaApi { .. }) => api_token.clone(),
            (
                Self::AzureServicePrincipal {
                    client_id,
                    client_secret,
                    tenant_id,
                },
                CredentialScope::AzureServicePrincipal { .. },
            ) => format!("{client_id}:{client_secret}:{tenant_id}"),
            (Self::SshGateway { gateway_id }, CredentialScope::SshSession { lease_id }) => {
                format!("{gateway_id}:{lease_id}")
            }
            _ => return Err(CredentialMaterialError::ScopeMismatch),
        };
        Ok(ShortLivedToken::new(value))
    }
}

impl fmt::Debug for CredentialMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct(self.kind()).finish_non_exhaustive()
    }
}
