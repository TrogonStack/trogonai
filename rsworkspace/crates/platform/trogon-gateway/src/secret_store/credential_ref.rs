use std::fmt;

use super::{CredentialId, CredentialKind, CredentialOwnerId, CredentialScope, CredentialVersion, SourceKind};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CredentialRef {
    id: CredentialId,
    version: CredentialVersion,
    owner_id: CredentialOwnerId,
    source: SourceKind,
    kind: CredentialKind,
}

impl CredentialRef {
    pub fn new(id: CredentialId, version: CredentialVersion, scope: &CredentialScope, kind: CredentialKind) -> Self {
        Self {
            id,
            version,
            owner_id: scope.owner_id().clone(),
            source: scope.source_kind(),
            kind,
        }
    }

    pub fn id(&self) -> &CredentialId {
        &self.id
    }

    pub fn version(&self) -> CredentialVersion {
        self.version
    }

    pub fn owner_id(&self) -> &CredentialOwnerId {
        &self.owner_id
    }

    pub fn source(&self) -> SourceKind {
        self.source
    }

    pub fn kind(&self) -> CredentialKind {
        self.kind
    }

    pub fn next_version(&self) -> Self {
        Self {
            id: self.id.clone(),
            version: self.version.next(),
            owner_id: self.owner_id.clone(),
            source: self.source,
            kind: self.kind,
        }
    }
}

impl fmt::Display for CredentialRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.id, self.version.get())
    }
}
