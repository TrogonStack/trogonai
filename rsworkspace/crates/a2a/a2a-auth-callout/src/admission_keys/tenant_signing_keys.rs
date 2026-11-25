use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fmt;

use super::account_public_key::TenantAccount;
use super::account_signing_key::AccountSigningKey;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TenantSigningKeysError {
    #[error("lists no tenant account")]
    Empty,
    #[error("lists tenant account {0} more than once")]
    Duplicate(TenantAccount),
}

#[derive(Clone)]
pub struct TenantSigningKeys(BTreeMap<TenantAccount, AccountSigningKey>);

impl TenantSigningKeys {
    pub fn new(
        entries: impl IntoIterator<Item = (TenantAccount, AccountSigningKey)>,
    ) -> Result<Self, TenantSigningKeysError> {
        let mut keys = BTreeMap::new();
        for (tenant, signing_key) in entries {
            match keys.entry(tenant) {
                Entry::Occupied(listed) => return Err(TenantSigningKeysError::Duplicate(listed.key().clone())),
                Entry::Vacant(slot) => {
                    slot.insert(signing_key);
                }
            }
        }
        if keys.is_empty() {
            return Err(TenantSigningKeysError::Empty);
        }
        Ok(Self(keys))
    }

    pub fn signing_key(&self, tenant: &TenantAccount) -> Option<&AccountSigningKey> {
        self.0.get(tenant)
    }

    pub fn tenants(&self) -> impl Iterator<Item = &TenantAccount> {
        self.0.keys()
    }
}

impl fmt::Debug for TenantSigningKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.0.iter()).finish()
    }
}
