use super::{
    CredentialId, CredentialIdentityVersion, CredentialKey, CredentialKind, CredentialOwnerId, CredentialScope,
    PublicCredentialId, SourceKind,
};

/// Everything a credential is, recorded once on the write request that opens its
/// stream.
///
/// Every later event names the credential by its key alone, and replay rebuilds
/// the identity from that write request instead of deriving the id again.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CredentialIdentity {
    id: CredentialId,
    scope: CredentialScope,
    kind: CredentialKind,
    version: CredentialIdentityVersion,
}

impl CredentialIdentity {
    /// Derives the identity of the slot a caller names. Only the management API
    /// derives one: everything downstream reads the identity it recorded.
    pub fn derive(scope: CredentialScope, kind: CredentialKind) -> Self {
        Self::new(
            CredentialId::derive(&scope, kind),
            scope,
            kind,
            CredentialIdentityVersion::V1,
        )
    }

    /// Rebuilds a recorded identity. The id is taken as recorded and never checked
    /// against the slot, because the rule that minted it need not be the current one.
    pub fn new(
        id: CredentialId,
        scope: CredentialScope,
        kind: CredentialKind,
        version: CredentialIdentityVersion,
    ) -> Self {
        Self {
            id,
            scope,
            kind,
            version,
        }
    }

    pub fn id(&self) -> &CredentialId {
        &self.id
    }

    pub fn scope(&self) -> &CredentialScope {
        &self.scope
    }

    pub fn owner_id(&self) -> &CredentialOwnerId {
        self.scope.owner_id()
    }

    pub fn source(&self) -> SourceKind {
        self.scope.source_kind()
    }

    pub fn scope_key(&self) -> String {
        self.scope.scope_key()
    }

    pub fn kind(&self) -> CredentialKind {
        self.kind
    }

    pub fn version(&self) -> CredentialIdentityVersion {
        self.version
    }

    pub fn key(&self) -> CredentialKey {
        CredentialKey::new(self.owner_id().clone(), self.id.clone())
    }

    pub fn public_id(&self) -> PublicCredentialId {
        PublicCredentialId::new(self.owner_id(), &self.scope_key(), self.kind)
    }
}

impl From<&CredentialIdentity> for CredentialKey {
    fn from(identity: &CredentialIdentity) -> Self {
        identity.key()
    }
}

#[cfg(test)]
mod tests;
