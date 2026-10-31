//! The principal a subject's `{kind}.{id}` tokens name, per
//! [ADR#0082](../../../../../docs/adr/0082-bus-principal-from-subject-and-edge-proof-of-possession.md).

use trogon_decider_runtime::{PrincipalId, PrincipalIdError, PrincipalKind};

/// A principal read from the two subject tokens that name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectPrincipal {
    kind: PrincipalKind,
    id: PrincipalId,
}

impl SubjectPrincipal {
    /// Reads a principal from its kind and id tokens, refusing either one when
    /// it is not a token the subject could have carried for a real principal.
    pub fn from_tokens(kind: &str, id: &str) -> Result<Self, SubjectPrincipalError> {
        let kind = [PrincipalKind::Agent, PrincipalKind::Person, PrincipalKind::Service]
            .into_iter()
            .find(|candidate| candidate.as_str() == kind)
            .ok_or_else(|| SubjectPrincipalError::Kind { token: kind.to_owned() })?;
        if id.is_empty()
            || id
                .chars()
                .any(|character| matches!(character, '.' | '*' | '>') || character.is_whitespace())
        {
            return Err(SubjectPrincipalError::IdToken { token: id.to_owned() });
        }
        let id = PrincipalId::new(id).map_err(|source| SubjectPrincipalError::Id {
            token: id.to_owned(),
            source,
        })?;

        Ok(Self { kind, id })
    }

    /// The kind the subject named.
    pub const fn kind(&self) -> PrincipalKind {
        self.kind
    }

    /// The id the subject named.
    pub fn principal_id(&self) -> PrincipalId {
        self.id.clone()
    }
}

/// Why a pair of subject tokens does not name a principal.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SubjectPrincipalError {
    /// The kind token is not the wire name of any principal kind.
    #[error("principal kind token {token:?} is not a principal kind")]
    Kind { token: String },
    /// The id token is empty or carries a dot, a wildcard, or whitespace.
    #[error("principal id token {token:?} is empty or carries a dot, a wildcard or whitespace")]
    IdToken { token: String },
    /// The id token is not a principal id.
    #[error("principal id token {token:?} is not a principal id: {source}")]
    Id {
        token: String,
        #[source]
        source: PrincipalIdError,
    },
}
