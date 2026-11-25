use std::fmt;

use uuid::Uuid;

use super::constants::{CREDENTIAL_ID_LEN, CREDENTIAL_ID_NAMESPACE};
use super::{CredentialKind, CredentialScope};

/// Opaque identity of a credential aggregate.
///
/// The value carries no owner, scope, kind, or storage backend: it is a UUID in
/// its canonical lowercase hyphenated text. The management API derives it from
/// the slot a caller manages, because nothing yet keeps an index from that slot
/// to a minted id. Everything else reads the id recorded on the write request
/// that opened the credential's stream and never derives it again. The id is
/// unique only within its project, which is why `CredentialKey` pairs it with
/// the owner.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CredentialId(Uuid);

impl CredentialId {
    /// Derives the id the way `CREDENTIAL_IDENTITY_VERSION_V1` declares.
    pub(super) fn derive(scope: &CredentialScope, kind: CredentialKind) -> Self {
        Self(Uuid::new_v5(
            &CREDENTIAL_ID_NAMESPACE,
            derivation_name(scope, kind).as_bytes(),
        ))
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
        if let Some(ch) = value.chars().find(|ch| !matches!(ch, '0'..='9' | 'a'..='f' | '-')) {
            return Err(CredentialIdError::InvalidCharacter(ch));
        }
        Uuid::try_parse(value)
            .map(Self)
            .map_err(CredentialIdError::InvalidFormat)
    }
}

/// Owner ids and kinds never hold a slash and a scope key is a source with an
/// optional integration id, so the first slash ends the owner and the last one
/// starts the kind: no two slots share a name.
fn derivation_name(scope: &CredentialScope, kind: CredentialKind) -> String {
    format!("{}/{}/{}", scope.owner_id(), scope.scope_key(), kind.as_str())
}

impl fmt::Debug for CredentialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CredentialId").field(&self.to_string()).finish()
    }
}

impl fmt::Display for CredentialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0.hyphenated(), f)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialIdError {
    #[error("credential id must not be empty")]
    Empty,
    #[error("credential id contains invalid character '{0}'")]
    InvalidCharacter(char),
    #[error("credential id must be 36 characters, got {0}")]
    InvalidLength(usize),
    #[error("credential id is not a hyphenated UUID: {0}")]
    InvalidFormat(#[source] uuid::Error),
}

#[cfg(test)]
mod tests;
