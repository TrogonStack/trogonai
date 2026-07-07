use crate::source_integration_id::SourceIntegrationId;

use super::{CredentialOwnerId, SourceKind};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CredentialScope {
    Source {
        owner_id: CredentialOwnerId,
        source: SourceKind,
    },
    Integration {
        owner_id: CredentialOwnerId,
        source: SourceKind,
        integration_id: SourceIntegrationId,
    },
}

impl CredentialScope {
    pub fn source(owner_id: CredentialOwnerId, source: SourceKind) -> Self {
        Self::Source { owner_id, source }
    }

    pub fn integration(owner_id: CredentialOwnerId, source: SourceKind, integration_id: SourceIntegrationId) -> Self {
        Self::Integration {
            owner_id,
            source,
            integration_id,
        }
    }

    pub fn owner_id(&self) -> &CredentialOwnerId {
        match self {
            Self::Source { owner_id, .. } | Self::Integration { owner_id, .. } => owner_id,
        }
    }

    pub fn source_kind(&self) -> SourceKind {
        match self {
            Self::Source { source, .. } | Self::Integration { source, .. } => *source,
        }
    }

    pub fn scope_key(&self) -> String {
        match self {
            Self::Source { source, .. } => source.as_str().to_string(),
            Self::Integration {
                source, integration_id, ..
            } => format!("{}/{}", source.as_str(), integration_id.as_str()),
        }
    }
}
