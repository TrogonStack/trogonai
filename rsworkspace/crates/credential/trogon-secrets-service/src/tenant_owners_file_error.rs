use std::io;
use std::path::PathBuf;

use trogon_credential_domain::{CredentialOwnerId, CredentialOwnerIdError};

use super::{TenantAccount, TenantAccountError, TomlSyntax};

/// Why the tenant owners file was refused. The service does not start on any
/// of them: a tenant it cannot place would be denied every release anyway, and
/// a guess at what the operator meant could hand an owner to the wrong tenant.
#[derive(Debug, thiserror::Error)]
pub enum TenantOwnersFileError {
    #[error("tenant owners file {} could not be read", path.display())]
    Unreadable {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("tenant owners file {} is not a valid tenant owners file: {syntax}", path.display())]
    Unparsable { path: PathBuf, syntax: TomlSyntax },
    #[error("tenant owners file {} names no tenants", path.display())]
    NoTenants { path: PathBuf },
    #[error("tenant owners file {} names tenant {tenant:?}, which is not a tenant account", path.display())]
    InvalidTenant {
        path: PathBuf,
        tenant: String,
        #[source]
        source: TenantAccountError,
    },
    #[error("tenant {tenant} in tenant owners file {} holds no owners", path.display())]
    NoOwners { path: PathBuf, tenant: TenantAccount },
    #[error("tenant {tenant} in tenant owners file {} holds an invalid owner", path.display())]
    InvalidOwner {
        path: PathBuf,
        tenant: TenantAccount,
        #[source]
        source: CredentialOwnerIdError,
    },
    #[error("owner {owner} in tenant owners file {} is held by both {first} and {second}", path.display())]
    SharedOwner {
        path: PathBuf,
        owner: CredentialOwnerId,
        first: TenantAccount,
        second: TenantAccount,
    },
}
