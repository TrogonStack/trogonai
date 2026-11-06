use std::fmt;
use std::sync::Arc;

use uuid::Uuid;

use super::constants::{CREDENTIAL_ID_LEN, CREDENTIAL_ID_NAMESPACE};
use super::{CredentialKind, CredentialScope};

/// Opaque identity of a credential aggregate.
///
/// The value carries no owner, scope, kind, or storage backend: it is thirty-two
/// lowercase hex digits. It is derived rather than random because a credential
/// is addressed by `PublicCredentialId` (owner, scope, kind) and nothing keeps
/// an index from that composite to a minted id, so the same slot has to yield
/// the same id every time. The id is unique only within its project, which is
/// why the aggregate's stream identity pairs it with the owner.
#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub struct CredentialId(Arc<str>);

impl CredentialId {
    pub fn derive(scope: &CredentialScope, kind: CredentialKind) -> Self {
        let name = format!("{}\0{}\0{}", scope.owner_id(), scope.scope_key(), kind.as_str());
        let uuid = Uuid::new_v5(&CREDENTIAL_ID_NAMESPACE, name.as_bytes());
        Self(Arc::from(uuid.as_simple().to_string()))
    }

    pub fn new(value: impl AsRef<str>) -> Result<Self, CredentialIdError> {
        let value = value.as_ref();
        let char_count = value.chars().count();
        if char_count == 0 {
            return Err(CredentialIdError::Empty);
        }
        if char_count != CREDENTIAL_ID_LEN {
            return Err(CredentialIdError::InvalidLength(char_count));
        }
        for ch in value.chars() {
            if !matches!(ch, '0'..='9' | 'a'..='f') {
                return Err(CredentialIdError::InvalidCharacter(ch));
            }
        }
        Ok(Self(Arc::from(value)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CredentialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CredentialId").field(&self.as_str()).finish()
    }
}

impl fmt::Display for CredentialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialIdError {
    #[error("credential id must not be empty")]
    Empty,
    #[error("credential id contains invalid character '{0}'")]
    InvalidCharacter(char),
    #[error("credential id must be 32 characters, got {0}")]
    InvalidLength(usize),
}

#[cfg(test)]
mod tests;
