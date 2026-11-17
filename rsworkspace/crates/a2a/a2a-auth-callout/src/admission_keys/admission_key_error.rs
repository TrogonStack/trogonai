use std::fmt;

use trogon_nats::SettingError;
use trogon_service_config::SecretReferenceError;

use super::account_public_key::{AccountPublicKeyError, TenantAccount};
use super::account_signing_key::AccountSigningKeyError;
use super::callout_xkey::CalloutXkeyError;
use super::signing_key_custody::SigningKeyCustody;
use super::tenant_signing_keys::TenantSigningKeysError;
use crate::constants::{
    AUTH_ACCOUNT_VAR, CALLOUT_XKEY_VAR, RESPONSE_SIGNING_KEY_VAR, SIGNING_KEY_SOURCE_VAR, TENANT_SIGNING_KEYS_VAR,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionKeySlot {
    ResponseSigningKey,
    TenantSigningKey(TenantAccount),
    CalloutXkey,
}

impl fmt::Display for AdmissionKeySlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResponseSigningKey => f.write_str(RESPONSE_SIGNING_KEY_VAR),
            Self::TenantSigningKey(tenant) => write!(f, "{TENANT_SIGNING_KEYS_VAR} entry for {tenant}"),
            Self::CalloutXkey => f.write_str(CALLOUT_XKEY_VAR),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AdmissionKeyError {
    #[error("{var} {0}", var = AUTH_ACCOUNT_VAR)]
    InvalidAuthAccount(#[source] AccountPublicKeyError),
    #[error("{slot} {source}")]
    InvalidReference {
        slot: AdmissionKeySlot,
        #[source]
        source: SecretReferenceError,
    },
    #[error("{slot} must be a {custody}:// reference while {var} is {custody}", var = SIGNING_KEY_SOURCE_VAR)]
    ReferenceOutsideCustody {
        slot: AdmissionKeySlot,
        custody: SigningKeyCustody,
    },
    #[error("{slot} cannot be resolved: {source}")]
    Unresolvable {
        slot: AdmissionKeySlot,
        #[source]
        source: SettingError,
    },
    #[error("{slot} resolves to a value that {source}")]
    InvalidSigningKey {
        slot: AdmissionKeySlot,
        #[source]
        source: AccountSigningKeyError,
    },
    #[error("{var} resolves to a value that {0}", var = CALLOUT_XKEY_VAR)]
    InvalidCalloutXkey(#[source] CalloutXkeyError),
    #[error(
        "{var} entry {position} is not <tenant account public key>=<secret reference>",
        var = TENANT_SIGNING_KEYS_VAR
    )]
    MalformedTenantEntry { position: usize },
    #[error("{var} entry {position} names no tenant account: it {source}", var = TENANT_SIGNING_KEYS_VAR)]
    InvalidTenantAccount {
        position: usize,
        #[source]
        source: AccountPublicKeyError,
    },
    #[error("{var} {0}", var = TENANT_SIGNING_KEYS_VAR)]
    TenantSigningKeys(#[source] TenantSigningKeysError),
}
