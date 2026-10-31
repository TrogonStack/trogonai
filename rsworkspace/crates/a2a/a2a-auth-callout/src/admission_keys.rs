mod account_public_key;
mod account_signing_key;
mod admission_key_error;
mod callout_xkey;
mod loader;
mod signing_key_custody;
mod tenant_signing_keys;
mod xkey_public;

pub use account_public_key::{AccountPublicKeyError, AuthAccount, TenantAccount};
pub use account_signing_key::{AccountSigningKey, AccountSigningKeyError};
pub use admission_key_error::{AdmissionKeyError, AdmissionKeySlot};
pub use callout_xkey::{CalloutXkey, CalloutXkeyError, XkeyExchangeError};
pub use loader::admission_keys_from_env;
pub use signing_key_custody::SigningKeyCustody;
pub use tenant_signing_keys::{TenantSigningKeys, TenantSigningKeysError};
pub use xkey_public::XkeyPublic;

#[derive(Debug, Clone)]
pub struct AdmissionKeys {
    auth_account: AuthAccount,
    response_signing_key: AccountSigningKey,
    tenant_signing_keys: TenantSigningKeys,
    callout_xkey: Option<CalloutXkey>,
}

impl AdmissionKeys {
    pub(crate) fn new(
        auth_account: AuthAccount,
        response_signing_key: AccountSigningKey,
        tenant_signing_keys: TenantSigningKeys,
        callout_xkey: Option<CalloutXkey>,
    ) -> Self {
        Self {
            auth_account,
            response_signing_key,
            tenant_signing_keys,
            callout_xkey,
        }
    }

    pub fn auth_account(&self) -> &AuthAccount {
        &self.auth_account
    }

    pub fn response_signing_key(&self) -> &AccountSigningKey {
        &self.response_signing_key
    }

    pub fn tenant_signing_keys(&self) -> &TenantSigningKeys {
        &self.tenant_signing_keys
    }

    pub fn callout_xkey(&self) -> Option<&CalloutXkey> {
        self.callout_xkey.as_ref()
    }
}

#[cfg(test)]
mod tests;
