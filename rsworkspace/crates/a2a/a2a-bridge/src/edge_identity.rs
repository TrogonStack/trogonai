use std::fmt;

#[cfg(test)]
use base64::Engine;
#[cfg(test)]
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use trogon_aauth_verify::VerifiedPresenter;

pub use crate::credential_subject::{BearerSubjectError, CredentialSubject, CredentialSubjectError};
pub use crate::key_thumbprint::KeyThumbprint;

/// The AAuth token a caller proved possession of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentedToken {
    Agent,
    Auth,
}

impl PresentedToken {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Auth => "auth",
        }
    }
}

impl fmt::Display for PresentedToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

/// The caller the edge verified through proof of possession: who its token
/// names, which token it presented, and the key it signed the request with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedEdgeCaller {
    subject: CredentialSubject,
    token: PresentedToken,
    key_thumbprint: KeyThumbprint,
}

impl VerifiedEdgeCaller {
    /// # Errors
    ///
    /// [`CredentialSubjectError`] when the verified token names an empty
    /// issuer or subject, which leaves no caller to bind the request to.
    pub fn from_presenter(presenter: &VerifiedPresenter) -> Result<Self, CredentialSubjectError> {
        let (issuer, subject, token, jkt) = match presenter {
            VerifiedPresenter::Agent(agent) => {
                (&agent.claims.iss, &agent.claims.sub, PresentedToken::Agent, &agent.jkt)
            }
            VerifiedPresenter::Auth(auth) => (
                &auth.auth.claims.iss,
                &auth.auth.claims.sub,
                PresentedToken::Auth,
                &auth.jkt,
            ),
        };
        Ok(Self {
            subject: CredentialSubject::new(issuer.as_str(), subject.as_str())?,
            token,
            key_thumbprint: KeyThumbprint::new(jkt.clone()),
        })
    }

    #[must_use]
    pub fn subject(&self) -> &CredentialSubject {
        &self.subject
    }

    #[must_use]
    pub fn token(&self) -> PresentedToken {
        self.token
    }

    #[must_use]
    pub fn key_thumbprint(&self) -> &KeyThumbprint {
        &self.key_thumbprint
    }
}

#[cfg(test)]
mod tests;
