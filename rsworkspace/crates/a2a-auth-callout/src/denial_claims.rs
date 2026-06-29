use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, Header, encode};
use serde::Serialize;
use uuid::Uuid;

use crate::denial_reason::DenialReason;
use crate::error::AuthCalloutError;
use crate::jwt::SigningKey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalloutIssuer(String);

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum CalloutIssuerError {
    #[error("callout issuer must be non-empty")]
    Empty,
}

impl CalloutIssuer {
    pub fn new(issuer: impl Into<String>) -> Result<Self, CalloutIssuerError> {
        let s = issuer.into();
        if s.is_empty() {
            return Err(CalloutIssuerError::Empty);
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerAudience(String);

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ServerAudienceError {
    #[error("server audience must be non-empty")]
    Empty,
}

impl ServerAudience {
    pub fn new(audience: impl Into<String>) -> Result<Self, ServerAudienceError> {
        let s = audience.into();
        if s.is_empty() {
            return Err(ServerAudienceError::Empty);
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserNkeySubject(String);

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum UserNkeySubjectError {
    #[error("user nkey subject must be non-empty")]
    Empty,
}

impl UserNkeySubject {
    pub fn new(subject: impl Into<String>) -> Result<Self, UserNkeySubjectError> {
        let s = subject.into();
        if s.is_empty() {
            return Err(UserNkeySubjectError::Empty);
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct DenialClaims {
    pub iss: CalloutIssuer,
    pub aud: ServerAudience,
    pub sub: UserNkeySubject,
    pub reason: DenialReason,
    pub request_jti: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum DenialClaimsError {
    #[error("denial JWT encode error: {0}")]
    Encode(#[source] jsonwebtoken::errors::Error),
    #[error("system time error: {0}")]
    SystemTime(#[source] std::time::SystemTimeError),
    #[error("issued-at timestamp out of portable range")]
    IssuedAtOutOfRange,
}

impl From<DenialClaimsError> for AuthCalloutError {
    fn from(value: DenialClaimsError) -> Self {
        // Denial-claims minting failures aren't JwtError — they're this
        // module's own typed error. Wrap as Internal so the source chain
        // is preserved via the std::error::Error impl above.
        Self::Internal(value.to_string())
    }
}

impl DenialClaims {
    pub fn mint(
        &self,
        signing_key: &SigningKey,
        issued_at: SystemTime,
        ttl: Duration,
    ) -> Result<String, DenialClaimsError> {
        let iat_secs = secs_since_unix(issued_at)?;
        let ttl_secs_i64 = i64::try_from(ttl.as_secs().max(1)).unwrap_or(i64::MAX);
        let exp_secs = iat_secs.saturating_add(ttl_secs_i64);
        let jti = Uuid::new_v4().to_string();

        #[derive(Serialize)]
        struct NatsDenial<'a> {
            error: &'a str,
            #[serde(rename = "type")]
            typ: &'static str,
            version: u32,
        }

        #[derive(Serialize)]
        struct Claims<'a> {
            iss: &'a str,
            aud: &'a str,
            sub: &'a str,
            iat: i64,
            exp: i64,
            jti: String,
            nats: NatsDenial<'a>,
        }

        let claims = Claims {
            iss: self.iss.as_str(),
            aud: self.aud.as_str(),
            sub: self.sub.as_str(),
            iat: iat_secs,
            exp: exp_secs,
            jti,
            nats: NatsDenial {
                error: self.reason.as_str(),
                typ: "authorization_response",
                version: 2,
            },
        };

        encode(&Header::new(Algorithm::HS256), &claims, &signing_key.encoding_key()).map_err(DenialClaimsError::Encode)
    }

    #[cfg(test)]
    pub(crate) fn mint_for_test(&self, signing_key: &SigningKey, ttl: Duration) -> Result<String, DenialClaimsError> {
        self.mint(signing_key, UNIX_EPOCH + Duration::from_secs(1_000), ttl)
    }
}

fn secs_since_unix(t: SystemTime) -> Result<i64, DenialClaimsError> {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map_err(DenialClaimsError::SystemTime)?
        .as_secs();
    i64::try_from(secs).map_err(|_| DenialClaimsError::IssuedAtOutOfRange)
}

#[cfg(test)]
mod tests;
