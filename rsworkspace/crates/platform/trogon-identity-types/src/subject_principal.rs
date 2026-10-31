use std::fmt;

use crate::principal_id::PrincipalId;
use crate::principal_kind::{PrincipalKind, PrincipalKindError};
use crate::subject_principal_id::{SubjectPrincipalId, SubjectPrincipalIdError};

/// The principal position of a subject: the [`PrincipalKind`] wire name
/// token followed by the [`SubjectPrincipalId`] token.
///
/// Renders as those two tokens joined by `.`, ready to be spliced into a
/// subject.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubjectPrincipal {
    kind: PrincipalKind,
    id: SubjectPrincipalId,
}

impl SubjectPrincipal {
    /// Pairs a kind with an id already in subject form.
    pub const fn new(kind: PrincipalKind, id: SubjectPrincipalId) -> Self {
        Self { kind, id }
    }

    /// Parses the two tokens of a principal position taken from a subject.
    pub fn from_tokens(kind: &str, id: &str) -> Result<Self, SubjectPrincipalError> {
        Ok(Self {
            kind: kind.parse()?,
            id: SubjectPrincipalId::from_token(id)?,
        })
    }

    /// Places a principal id into subject form. Mapping an id that was
    /// derived from a subject returns that subject's principal unchanged.
    pub fn from_principal(kind: PrincipalKind, id: &PrincipalId) -> Self {
        Self {
            kind,
            id: SubjectPrincipalId::from(id),
        }
    }

    /// Places the principal a verified credential carries into subject form,
    /// refusing an id that already has the hashed form.
    pub fn from_credential(kind: PrincipalKind, id: &PrincipalId) -> Result<Self, SubjectPrincipalError> {
        Ok(Self {
            kind,
            id: SubjectPrincipalId::from_credential(id)?,
        })
    }

    /// Returns the kind of actor the subject names.
    pub const fn kind(&self) -> PrincipalKind {
        self.kind
    }

    /// Returns the id token the subject carries.
    pub const fn id(&self) -> &SubjectPrincipalId {
        &self.id
    }

    /// Returns the principal id a receiver derives from the subject.
    pub fn principal_id(&self) -> PrincipalId {
        self.id.principal_id()
    }
}

impl fmt::Display for SubjectPrincipal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.kind, self.id)
    }
}

/// Error returned when the tokens of a principal position do not name a
/// principal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SubjectPrincipalError {
    /// The kind token is not a principal kind wire name.
    #[error(transparent)]
    Kind(#[from] PrincipalKindError),
    /// The id token is not a principal id in subject form.
    #[error(transparent)]
    Id(#[from] SubjectPrincipalIdError),
}

#[cfg(test)]
mod tests;
