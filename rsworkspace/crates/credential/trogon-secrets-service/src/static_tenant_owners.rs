use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use trogon_credential_domain::CredentialOwnerId;
use trogon_std::ReadFile;

use super::tenant_owners_file_wire::TenantOwnersFileWire;
use super::{OwnerHolding, OwnerTenants, TenantAccount, TenantOwners, TenantOwnersFileError, TomlSyntax};

/// Tenant owners loaded once at startup.
///
/// Each owner has one holder, kept by owner, so the forward and reverse
/// questions answer from the same record and can never disagree.
#[derive(Clone, Debug, Default)]
pub struct StaticTenantOwners {
    tenants: BTreeSet<TenantAccount>,
    holders: BTreeMap<CredentialOwnerId, TenantAccount>,
}

impl StaticTenantOwners {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that `tenant` holds `owner`.
    ///
    /// An owner already held moves to `tenant`, and the tenant it leaves stays
    /// known, so an owner never has two holders that could both receive its
    /// credentials.
    #[must_use]
    pub fn with_owner(mut self, tenant: TenantAccount, owner: CredentialOwnerId) -> Self {
        self.tenants.insert(tenant.clone());
        self.holders.insert(owner, tenant);
        self
    }

    /// Reads the tenant owners file at `path`, once, at startup.
    ///
    /// The file names each tenant by its NATS account key, the token a release
    /// subject carries, with the credential owners it holds:
    ///
    /// ```toml
    /// [tenants]
    /// "ACCOUNT_KEY_OF_TENANT_A" = ["project-a", "project-b"]
    /// ```
    ///
    /// An owner belongs to one tenant, so a file that gives one owner to two
    /// tenants is refused rather than letting both receive its credentials.
    ///
    /// # Errors
    ///
    /// Returns [`TenantOwnersFileError`] when the file cannot be read or
    /// parsed, names no tenants, or names a tenant or owner it cannot keep.
    pub fn load(path: &Path, fs: &impl ReadFile) -> Result<Self, TenantOwnersFileError> {
        let document = fs
            .read_to_string(path)
            .map_err(|source| TenantOwnersFileError::Unreadable {
                path: path.to_owned(),
                source,
            })?;
        let wire: TenantOwnersFileWire =
            toml::from_str(&document).map_err(|error| TenantOwnersFileError::Unparsable {
                path: path.to_owned(),
                syntax: TomlSyntax::of(&error, &document),
            })?;
        if wire.tenants.is_empty() {
            return Err(TenantOwnersFileError::NoTenants { path: path.to_owned() });
        }
        let mut loaded = Self::new();
        for (named, owners) in wire.tenants {
            let tenant = TenantAccount::parse(&named).map_err(|source| TenantOwnersFileError::InvalidTenant {
                path: path.to_owned(),
                tenant: named,
                source,
            })?;
            if owners.is_empty() {
                return Err(TenantOwnersFileError::NoOwners {
                    path: path.to_owned(),
                    tenant,
                });
            }
            for owner in owners {
                let owner = CredentialOwnerId::new(owner).map_err(|source| TenantOwnersFileError::InvalidOwner {
                    path: path.to_owned(),
                    tenant: tenant.clone(),
                    source,
                })?;
                if let Some(first) = loaded.holders.get(&owner).cloned()
                    && first != tenant
                {
                    return Err(TenantOwnersFileError::SharedOwner {
                        path: path.to_owned(),
                        owner,
                        first,
                        second: tenant,
                    });
                }
                loaded = loaded.with_owner(tenant.clone(), owner);
            }
        }
        Ok(loaded)
    }
}

impl TenantOwners for StaticTenantOwners {
    fn holding(&self, tenant: &TenantAccount, owner: &CredentialOwnerId) -> OwnerHolding {
        if !self.tenants.contains(tenant) {
            OwnerHolding::UnknownTenant
        } else if self.holders.get(owner) == Some(tenant) {
            OwnerHolding::Held
        } else {
            OwnerHolding::NotHeld
        }
    }
}

impl OwnerTenants for StaticTenantOwners {
    fn holder(&self, owner: &CredentialOwnerId) -> Option<TenantAccount> {
        self.holders.get(owner).cloned()
    }
}

#[cfg(test)]
mod tests;
