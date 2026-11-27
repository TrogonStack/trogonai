use trogon_sandbox_contract::{ClaimName, EgressViolationReason};

use crate::JwtDefect;

/// Why a JWT credential could not be masked. Errors name the defect, never the content.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum JwtMaskError {
    #[error("the value is not a verifiable JWT: {0}")]
    Malformed(JwtDefect),
    #[error("none of the named claims is present as a string claim in the JWT")]
    NoClaimMatched,
    #[error("claim '{0}' holds a control character, which cannot be substituted into HTTP safely")]
    UnsafeClaimValue(ClaimName),
    #[error("the name for claim {index} of '{binding}' is not a valid secret name")]
    ClaimEntryName {
        binding: trogon_sandbox_contract::SecretName,
        index: usize,
    },
}

impl JwtMaskError {
    /// The refusal of a derived name for masked claim `index` of `binding`.
    pub(crate) fn entry_name(binding: &trogon_sandbox_contract::SecretName, index: usize) -> Self {
        Self::ClaimEntryName {
            binding: binding.clone(),
            index,
        }
    }

    /// The egress violation reason the refusal is reported under.
    pub fn violation_reason(&self) -> EgressViolationReason {
        match self {
            Self::NoClaimMatched => EgressViolationReason::SecretJwtClaimsUnmatched,
            Self::Malformed(_) | Self::UnsafeClaimValue(_) | Self::ClaimEntryName { .. } => {
                EgressViolationReason::SecretJwtMalformed
            }
        }
    }
}
