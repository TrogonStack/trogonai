use trogon_credential_domain::{CredentialId, CredentialOwnerId};

use super::constants::INVALIDATION_DOMAIN;
use super::{ReleaseCacheKey, TenantAccount};

/// Which held releases one invalidation notice drops, read from its subject
/// `secrets.invalidation.v1.{tenant}.{owner_token}.{credential_id}`.
///
/// The payload is never read: a notice drops every copy of its credential,
/// for every host and location, whatever the reason it gives. A subject that
/// names no credential this client can read drops every copy it holds, so a
/// notice it cannot read never leaves a revoked value served.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ReleaseInvalidation {
    Credential {
        tenant: TenantAccount,
        owner_token: String,
        credential_id: CredentialId,
    },
    Unreadable,
}

impl ReleaseInvalidation {
    pub(crate) fn from_subject(subject: &str) -> Self {
        let tokens: Vec<&str> = subject.split('.').collect();
        let [domain @ .., tenant, owner_token, credential_id] = tokens.as_slice() else {
            return Self::Unreadable;
        };
        if domain != INVALIDATION_DOMAIN || owner_token.is_empty() {
            return Self::Unreadable;
        }
        let (Ok(tenant), Ok(credential_id)) = (TenantAccount::parse(tenant), CredentialId::new(credential_id)) else {
            return Self::Unreadable;
        };
        Self::Credential {
            tenant,
            owner_token: (*owner_token).to_owned(),
            credential_id,
        }
    }

    /// Whether the copy held under `key` has to go.
    pub(crate) fn covers(&self, key: &ReleaseCacheKey) -> bool {
        match self {
            Self::Credential {
                tenant,
                owner_token,
                credential_id,
            } => {
                key.tenant() == tenant
                    && key.credential().credential_id() == credential_id
                    && token_of(key.credential().owner_id()) == *owner_token
            }
            Self::Unreadable => true,
        }
    }
}

/// The owner id as the credential subjects write it: each `.` as `~`.
fn token_of(owner: &CredentialOwnerId) -> String {
    owner.as_str().replace('.', "~")
}

#[cfg(test)]
mod tests;
