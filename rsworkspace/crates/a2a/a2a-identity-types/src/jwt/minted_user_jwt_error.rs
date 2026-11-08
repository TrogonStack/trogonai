use trogon_identity_types::{PrincipalIdError, PrincipalKindError, SubjectPrincipalError};

use super::minted_claim::MintedClaim;
use crate::error::JwtError;

/// Error returned when a minted user JWT does not carry a caller the bus can
/// address.
#[derive(Debug, thiserror::Error)]
pub enum MintedUserJwtError {
    /// The payload segment does not decode to a JSON document.
    #[error("minted user JWT payload cannot be read")]
    Unreadable(#[from] JwtError),
    /// The claim is absent or is not a string.
    #[error("minted user JWT has no {0} claim")]
    MissingClaim(MintedClaim),
    /// The `caller_id` claim cannot be a single subject token.
    #[error("minted user JWT caller_id is not a caller id")]
    InvalidCallerId,
    /// The `caller_id` claim cannot name a principal.
    #[error("minted user JWT caller_id is not a principal id")]
    InvalidPrincipalId(#[source] PrincipalIdError),
    /// The `principal_kind` claim is not a principal kind wire name.
    #[error("minted user JWT principal_kind is not a principal kind")]
    InvalidPrincipalKind(#[source] PrincipalKindError),
    /// The caller cannot take a principal position in a subject.
    #[error("minted user JWT caller cannot take a subject principal position")]
    InvalidSubjectPrincipal(#[source] SubjectPrincipalError),
}

#[cfg(test)]
mod tests;
