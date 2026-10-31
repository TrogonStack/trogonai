use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use trogon_aauth_verify::VerifiedPresenter;

/// Who a credential names: its issuer and the subject that issuer vouches for.
/// Two credentials name the same caller only when both match exactly, since
/// the auth callout derives the caller it mints from exactly this pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialSubject {
    issuer: String,
    subject: String,
}

impl CredentialSubject {
    /// # Errors
    ///
    /// [`CredentialSubjectError`] when the issuer or the subject is empty.
    pub fn new(issuer: impl Into<String>, subject: impl Into<String>) -> Result<Self, CredentialSubjectError> {
        let issuer = issuer.into();
        let subject = subject.into();
        if issuer.is_empty() {
            return Err(CredentialSubjectError::EmptyIssuer);
        }
        if subject.is_empty() {
            return Err(CredentialSubjectError::EmptySubject);
        }
        Ok(Self { issuer, subject })
    }

    /// Reads `iss` and `sub` from a compact JWT without verifying it, which is
    /// only sound for comparing against a caller verified some other way.
    ///
    /// # Errors
    ///
    /// [`BearerSubjectError`] when the token is not a compact JWT whose payload
    /// names a non-empty issuer and subject.
    pub fn from_unverified_jwt(token: &str) -> Result<Self, BearerSubjectError> {
        let mut segments = token.split('.');
        let (Some(_), Some(payload), Some(_), None) =
            (segments.next(), segments.next(), segments.next(), segments.next())
        else {
            return Err(BearerSubjectError::NotCompactJwt);
        };
        let payload = URL_SAFE_NO_PAD.decode(payload).map_err(BearerSubjectError::Encoding)?;
        let claims: SubjectClaims = serde_json::from_slice(&payload).map_err(BearerSubjectError::Claims)?;
        Self::new(claims.iss, claims.sub).map_err(BearerSubjectError::Subject)
    }

    #[must_use]
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }
}

impl fmt::Display for CredentialSubject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.subject, self.issuer)
    }
}

#[derive(Deserialize)]
struct SubjectClaims {
    iss: String,
    sub: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CredentialSubjectError {
    #[error("credential names no issuer")]
    EmptyIssuer,
    #[error("credential names no subject")]
    EmptySubject,
}

#[derive(Debug, thiserror::Error)]
pub enum BearerSubjectError {
    #[error("Authorization does not carry a bearer token")]
    NotBearer,
    #[error("bearer token is not a compact JWT")]
    NotCompactJwt,
    #[error("bearer token payload is not base64url")]
    Encoding(#[source] base64::DecodeError),
    #[error("bearer token payload does not name an issuer and subject")]
    Claims(#[source] serde_json::Error),
    #[error("bearer token names no usable caller")]
    Subject(#[source] CredentialSubjectError),
}

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

/// RFC 7638 thumbprint of the key a caller signed its request with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyThumbprint(String);

impl KeyThumbprint {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for KeyThumbprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
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
            key_thumbprint: KeyThumbprint(jkt.clone()),
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
