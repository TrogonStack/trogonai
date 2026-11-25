use crate::credential_expiry::CredentialExpiry;
use crate::jwt::UserJwtClaims;

/// Ephemeral verification evidence; the source deadline never becomes a wire claim.
#[derive(Debug, Clone)]
pub struct VerifiedCredential {
    claims: UserJwtClaims,
    expires_at: CredentialExpiry,
}

impl VerifiedCredential {
    pub fn new(claims: UserJwtClaims, expires_at: CredentialExpiry) -> Self {
        Self { claims, expires_at }
    }

    pub fn claims(&self) -> &UserJwtClaims {
        &self.claims
    }

    pub fn expires_at(&self) -> CredentialExpiry {
        self.expires_at
    }

    pub(crate) fn into_parts(self) -> (UserJwtClaims, CredentialExpiry) {
        (self.claims, self.expires_at)
    }
}
