use std::env::VarError;

use trogon_secret_store::{OpenBaoAppRole, OpenBaoAppRoleError};
use trogon_service_config::SecretReference;
use trogon_std::env::ReadEnv;
use trogon_std::{ReadFile, SecretString};

use super::{ConfigSetting, ReleaseSourcesError, ServiceConfig, StaticReleasePolicies, StaticTenantOwners, XkeySealer};

/// What the service reads once at startup to build its release core: the
/// tenant owners and the release policies it decides with, the key it seals
/// with, and the AppRole it logs in to OpenBao with.
///
/// Each is read from where the [`ServiceConfig`] names it. The sealing key
/// seed and the AppRole secret id are never in the configuration itself: the
/// seed is read through its reference (ADR#0083) and the secret id from its
/// file (ADR#0023 Decision 3).
#[derive(Clone, Debug)]
pub struct ReleaseSources {
    pub tenant_owners: StaticTenantOwners,
    pub policies: StaticReleasePolicies,
    pub sealer: XkeySealer,
    pub approle: OpenBaoAppRole,
}

impl ReleaseSources {
    /// Reads every source `config` names, the sealing key seed from the
    /// variable or the file its reference names, trimmed of surrounding
    /// whitespace such as a trailing newline.
    ///
    /// # Errors
    ///
    /// Returns [`ReleaseSourcesError`], naming the setting, when a file cannot
    /// be read or is invalid, when the seed is unset, empty or not a curve
    /// seed, or when the AppRole cannot be read. No error quotes the seed or
    /// the secret id.
    pub fn load(config: &ServiceConfig, env: &impl ReadEnv, fs: &impl ReadFile) -> Result<Self, ReleaseSourcesError> {
        let tenant_owners =
            StaticTenantOwners::load(&config.tenant_owners_file, fs).map_err(ReleaseSourcesError::TenantOwners)?;
        let policies = StaticReleasePolicies::load(&config.release_policies_file, fs)
            .map_err(ReleaseSourcesError::ReleasePolicies)?;
        let seed = read_seed(&config.xkey_seed, env, fs)?;
        let sealer = XkeySealer::from_seed(&seed).map_err(|source| ReleaseSourcesError::XkeySeed {
            reference: config.xkey_seed.clone(),
            source,
        })?;
        let approle = OpenBaoAppRole::from_secret_id_file(&config.openbao_role_id, &config.openbao_secret_id_file, fs)
            .map_err(|source| ReleaseSourcesError::AppRole {
                setting: match source {
                    OpenBaoAppRoleError::EmptyRoleId => ConfigSetting::OpenBaoRoleId,
                    OpenBaoAppRoleError::SecretIdUnreadable { .. } | OpenBaoAppRoleError::EmptySecretId { .. } => {
                        ConfigSetting::OpenBaoSecretIdFile
                    }
                },
                source,
            })?;
        Ok(Self {
            tenant_owners,
            policies,
            sealer,
            approle,
        })
    }
}

fn read_seed(
    reference: &SecretReference,
    env: &impl ReadEnv,
    fs: &impl ReadFile,
) -> Result<SecretString, ReleaseSourcesError> {
    let raw = match reference {
        SecretReference::Env(variable) => env.var(variable.as_str()).map_err(|error| match error {
            VarError::NotPresent => ReleaseSourcesError::XkeySeedUnset {
                variable: variable.clone(),
            },
            VarError::NotUnicode(_) => ReleaseSourcesError::XkeySeedNotUnicode {
                variable: variable.clone(),
            },
        })?,
        SecretReference::File(path) => {
            fs.read_to_string(path)
                .map_err(|source| ReleaseSourcesError::XkeySeedUnreadable {
                    path: path.clone(),
                    source,
                })?
        }
    };
    SecretString::new(raw.trim()).map_err(|_| ReleaseSourcesError::XkeySeedEmpty {
        reference: reference.clone(),
    })
}

#[cfg(test)]
mod tests;
