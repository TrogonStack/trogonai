use std::io;
use std::path::PathBuf;

use trogon_secret_store::OpenBaoAppRoleError;
use trogon_service_config::{EnvVarName, SecretReference};

use super::{ConfigSetting, ReleasePoliciesFileError, TenantOwnersFileError, XkeySealerError};

/// Why a source the release core is built from could not be read. The service
/// does not start on any of them. Each names the setting that points at the
/// source, and none quotes the sealing key seed or the AppRole secret id.
#[derive(Debug, thiserror::Error)]
pub enum ReleaseSourcesError {
    #[error("the tenant owners {} names could not be loaded", ConfigSetting::TenantOwnersFile)]
    TenantOwners(#[source] TenantOwnersFileError),
    #[error(
        "the release policies {} names could not be loaded",
        ConfigSetting::ReleasePoliciesFile
    )]
    ReleasePolicies(#[source] ReleasePoliciesFileError),
    #[error("{} names variable {variable}, which is not set", ConfigSetting::XkeySeed)]
    XkeySeedUnset { variable: EnvVarName },
    #[error("{} names variable {variable}, which is not valid UTF-8", ConfigSetting::XkeySeed)]
    XkeySeedNotUnicode { variable: EnvVarName },
    #[error("{} names file {}, which could not be read", ConfigSetting::XkeySeed, path.display())]
    XkeySeedUnreadable {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{} names {reference}, which is empty", ConfigSetting::XkeySeed)]
    XkeySeedEmpty { reference: SecretReference },
    #[error("{} names {reference}, which does not hold a usable seed", ConfigSetting::XkeySeed)]
    XkeySeed {
        reference: SecretReference,
        #[source]
        source: XkeySealerError,
    },
    #[error("{setting} cannot be used to log in to OpenBao")]
    AppRole {
        setting: ConfigSetting,
        #[source]
        source: OpenBaoAppRoleError,
    },
}
