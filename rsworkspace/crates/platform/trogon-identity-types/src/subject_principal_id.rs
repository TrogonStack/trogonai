use std::fmt;
use std::str::FromStr;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

use crate::principal_id::PrincipalId;

/// A principal id in the form a NATS subject carries it.
///
/// Per ADR#0082 the bus derives a principal from the subject it was delivered
/// on, and the auth callout admits a publish only on subjects that embed the
/// caller's own id. The id in the subject, the id in the grant and the id a
/// receiver derives must therefore be the same string, and this mapping is
/// the one way any of them is produced from a [`PrincipalId`].
///
/// # Scheme
///
/// - An id of 1 to 64 characters, each a graphic ASCII character (`!` through
///   `~`) other than `.`, `*` and `>`, that does not start with `~`, is used
///   verbatim.
/// - Every other id becomes `~` followed by the unpadded base64url encoding of
///   the SHA-256 digest of its UTF-8 bytes, 44 characters in all.
///
/// Either way the token is non-empty and carries no `.`, `*`, `>`,
/// whitespace or control character, so it is always exactly one subject
/// token and never a wildcard.
///
/// # Stability
///
/// The mapping is a pure function of the id's bytes. It takes no key, salt or
/// configuration, so every process, host and release maps an id to the same
/// token. Changing the scheme renames every hashed principal and has to be
/// rolled out like renaming them.
///
/// # Collisions
///
/// A verbatim token never starts with `~` and a hashed one always does, so a
/// verbatim id never collides with a hashed one. The digest is never
/// truncated, so two hashed ids share a token only on a SHA-256 collision.
///
/// The hashed form belongs to the mapping: an id that already has it maps to
/// itself, which keeps re-deriving the token from a principal id parsed out of
/// a subject idempotent. A credential whose own id had that form could
/// therefore claim the token of whichever id hashes to it, so the edge that
/// turns a verified credential into a grant or a subject maps through
/// [`SubjectPrincipalId::from_credential`], which refuses such an id. A
/// `~`-led token that is not a whole digest is not an output of the mapping
/// and never parses.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubjectPrincipalId(String);

impl SubjectPrincipalId {
    const MAX_VERBATIM_LEN: usize = 64;
    const HASHED_MARKER: char = '~';
    const DIGEST_LEN: usize = 32;

    /// Parses a token taken from a subject, accepting only what the mapping
    /// can produce.
    pub fn from_token(token: &str) -> Result<Self, SubjectPrincipalIdError> {
        TokenForm::of(token).map(|_| Self(token.to_owned()))
    }

    /// Maps the principal id a verified credential carries, refusing an id
    /// that already has the hashed form.
    pub fn from_credential(id: &PrincipalId) -> Result<Self, SubjectPrincipalIdError> {
        match TokenForm::of(id.as_str()) {
            Ok(TokenForm::Hashed) => Err(SubjectPrincipalIdError::AlreadyHashed),
            Ok(TokenForm::Verbatim) => Ok(Self(id.as_str().to_owned())),
            Err(_) => Ok(Self::hashed(id.as_str())),
        }
    }

    /// Returns the token as it appears in a subject.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns whether the token is the digest of the id rather than the id.
    pub fn is_hashed(&self) -> bool {
        self.0.starts_with(Self::HASHED_MARKER)
    }

    /// Returns the principal id a receiver derives from the subject: the
    /// token itself.
    pub fn principal_id(&self) -> PrincipalId {
        PrincipalId::from_subject_token(&self.0)
    }

    fn hashed(id: &str) -> Self {
        let mut token = String::from(Self::HASHED_MARKER);
        URL_SAFE_NO_PAD.encode_string(Sha256::digest(id.as_bytes()), &mut token);
        Self(token)
    }
}

enum TokenForm {
    Verbatim,
    Hashed,
}

impl TokenForm {
    fn of(token: &str) -> Result<Self, SubjectPrincipalIdError> {
        if token.is_empty() {
            return Err(SubjectPrincipalIdError::Empty);
        }
        if let Some(invalid) = token.chars().find(|c| !is_token_char(*c)) {
            return Err(SubjectPrincipalIdError::InvalidCharacter(invalid));
        }
        if let Some(digest) = token.strip_prefix(SubjectPrincipalId::HASHED_MARKER) {
            return match URL_SAFE_NO_PAD.decode(digest) {
                Ok(bytes) if bytes.len() == SubjectPrincipalId::DIGEST_LEN => Ok(Self::Hashed),
                _ => Err(SubjectPrincipalIdError::MalformedDigest),
            };
        }
        if token.len() > SubjectPrincipalId::MAX_VERBATIM_LEN {
            return Err(SubjectPrincipalIdError::TooLong);
        }
        Ok(Self::Verbatim)
    }
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_graphic() && !matches!(c, '.' | '*' | '>')
}

impl From<&PrincipalId> for SubjectPrincipalId {
    fn from(id: &PrincipalId) -> Self {
        match TokenForm::of(id.as_str()) {
            Ok(_) => Self(id.as_str().to_owned()),
            Err(_) => Self::hashed(id.as_str()),
        }
    }
}

impl From<PrincipalId> for SubjectPrincipalId {
    fn from(id: PrincipalId) -> Self {
        Self::from(&id)
    }
}

impl FromStr for SubjectPrincipalId {
    type Err = SubjectPrincipalIdError;

    fn from_str(token: &str) -> Result<Self, Self::Err> {
        Self::from_token(token)
    }
}

impl TryFrom<&str> for SubjectPrincipalId {
    type Error = SubjectPrincipalIdError;

    fn try_from(token: &str) -> Result<Self, Self::Error> {
        Self::from_token(token)
    }
}

impl fmt::Display for SubjectPrincipalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl AsRef<str> for SubjectPrincipalId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Error returned when a token or a credential id cannot be a
/// [`SubjectPrincipalId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SubjectPrincipalIdError {
    /// The token was empty.
    #[error("subject principal id cannot be empty")]
    Empty,
    /// The token holds a character no subject token may carry: `.`, `*`,
    /// `>`, whitespace, a control character or anything outside ASCII.
    #[error("subject principal id cannot contain {0:?}")]
    InvalidCharacter(char),
    /// The token is longer than any id the mapping keeps verbatim.
    #[error(
        "subject principal id cannot be longer than {} characters",
        SubjectPrincipalId::MAX_VERBATIM_LEN
    )]
    TooLong,
    /// The token starts with `~` but is not a whole SHA-256 digest in
    /// unpadded base64url.
    #[error("subject principal id starting with `~` must be a whole SHA-256 digest in unpadded base64url")]
    MalformedDigest,
    /// The credential's principal id already has the hashed form the mapping
    /// reserves for its own output.
    #[error("credential principal id already has the hashed subject form")]
    AlreadyHashed,
}

#[cfg(test)]
mod tests;
