use std::fmt;

use super::{
    CredentialId, CredentialIdentity, CredentialKey, CredentialKind, CredentialOwnerId, CredentialScope,
    CredentialVersion, PublicCredentialId, SourceKind,
};

/// One version of a credential's material.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CredentialRef {
    identity: CredentialIdentity,
    version: CredentialVersion,
}

impl CredentialRef {
    pub fn new(identity: CredentialIdentity, version: CredentialVersion) -> Self {
        Self { identity, version }
    }

    pub fn identity(&self) -> &CredentialIdentity {
        &self.identity
    }

    pub fn key(&self) -> CredentialKey {
        self.identity.key()
    }

    pub fn id(&self) -> &CredentialId {
        self.identity.id()
    }

    pub fn version(&self) -> CredentialVersion {
        self.version
    }

    pub fn owner_id(&self) -> &CredentialOwnerId {
        self.identity.owner_id()
    }

    pub fn scope(&self) -> &CredentialScope {
        self.identity.scope()
    }

    pub fn source(&self) -> SourceKind {
        self.identity.source()
    }

    pub fn scope_key(&self) -> String {
        self.identity.scope_key()
    }

    pub fn kind(&self) -> CredentialKind {
        self.identity.kind()
    }

    pub fn is_scoped_to(&self, scope: &CredentialScope) -> bool {
        self.identity.scope() == scope
    }

    pub fn public_id(&self) -> PublicCredentialId {
        self.identity.public_id()
    }

    pub fn next_version(&self) -> Self {
        self.with_version(self.version.next())
    }

    pub fn with_version(&self, version: CredentialVersion) -> Self {
        Self::new(self.identity.clone(), version)
    }
}

impl From<&CredentialRef> for CredentialKey {
    fn from(credential: &CredentialRef) -> Self {
        credential.key()
    }
}

impl fmt::Display for CredentialRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.identity.id(), self.version.get())
    }
}

#[cfg(test)]
mod tests;
