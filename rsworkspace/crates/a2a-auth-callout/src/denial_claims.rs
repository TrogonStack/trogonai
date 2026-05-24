use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{encode, Algorithm, Header};
use serde::Serialize;
use uuid::Uuid;

use crate::denial_reason::DenialReason;
use crate::error::AuthCalloutError;
use crate::jwt::SigningKey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalloutIssuer(String);

#[derive(Debug, PartialEq, Eq)]
pub enum CalloutIssuerError {
    Empty,
}

impl fmt::Display for CalloutIssuerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("callout issuer must be non-empty"),
        }
    }
}

impl std::error::Error for CalloutIssuerError {}

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

#[derive(Debug, PartialEq, Eq)]
pub enum ServerAudienceError {
    Empty,
}

impl fmt::Display for ServerAudienceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("server audience must be non-empty"),
        }
    }
}

impl std::error::Error for ServerAudienceError {}

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

#[derive(Debug, PartialEq, Eq)]
pub enum UserNkeySubjectError {
    Empty,
}

impl fmt::Display for UserNkeySubjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("user nkey subject must be non-empty"),
        }
    }
}

impl std::error::Error for UserNkeySubjectError {}

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

#[derive(Debug)]
pub enum DenialClaimsError {
    Encode(jsonwebtoken::errors::Error),
    SystemTime(std::time::SystemTimeError),
    IssuedAtOutOfRange,
}

impl fmt::Display for DenialClaimsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encode(e) => write!(f, "denial JWT encode error: {e}"),
            Self::SystemTime(e) => write!(f, "system time error: {e}"),
            Self::IssuedAtOutOfRange => f.write_str("issued-at timestamp out of portable range"),
        }
    }
}

impl std::error::Error for DenialClaimsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Encode(e) => Some(e),
            Self::SystemTime(e) => Some(e),
            _ => None,
        }
    }
}

impl From<DenialClaimsError> for AuthCalloutError {
    fn from(value: DenialClaimsError) -> Self {
        Self::JwtMint(value.to_string())
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
    pub(crate) fn mint_for_test(
        &self,
        signing_key: &SigningKey,
        ttl: Duration,
    ) -> Result<String, DenialClaimsError> {
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
mod tests {
    use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
    use serde::Deserialize;

    use super::*;
    use crate::denial_category::DenialCategory;

    #[derive(Debug, Deserialize)]
    struct ParsedDenial {
        iss: String,
        aud: String,
        sub: String,
        nats: ParsedNats,
    }

    #[derive(Debug, Deserialize)]
    struct ParsedNats {
        error: String,
        #[serde(rename = "type")]
        typ: String,
        version: u32,
        jwt: Option<String>,
    }

    fn sample_claims() -> DenialClaims {
        DenialClaims {
            iss: CalloutIssuer::new("ACALLOUTISSUER").unwrap(),
            aud: ServerAudience::new("ASERVERPUBKEY").unwrap(),
            sub: UserNkeySubject::new("UCLIENTNKEY").unwrap(),
            reason: DenialReason::new(DenialCategory::InvalidCredentials).unwrap(),
            request_jti: Some("REQJTI123".into()),
        }
    }

    #[test]
    fn denial_jwt_round_trip() {
        let signing_key = SigningKey::from_secret(b"denial-test-secret--------------");
        let claims = sample_claims();
        let token = claims
            .mint_for_test(&signing_key, Duration::from_secs(60))
            .unwrap();

        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = false;
        validation.validate_aud = false;
        let decoded = decode::<ParsedDenial>(
            &token,
            &DecodingKey::from_secret(b"denial-test-secret--------------"),
            &validation,
        )
        .unwrap();

        assert_eq!(decoded.claims.iss, "ACALLOUTISSUER");
        assert_eq!(decoded.claims.aud, "ASERVERPUBKEY");
        assert_eq!(decoded.claims.sub, "UCLIENTNKEY");
        assert_eq!(decoded.claims.nats.error, "invalid_credentials");
        assert_eq!(decoded.claims.nats.typ, "authorization_response");
        assert_eq!(decoded.claims.nats.version, 2);
        assert!(decoded.claims.nats.jwt.is_none());
    }

    #[test]
    fn denial_jwt_rejects_wrong_signing_key() {
        let signing_key = SigningKey::from_secret(b"signer-a------------------------");
        let token = sample_claims()
            .mint_for_test(&signing_key, Duration::from_secs(60))
            .unwrap();
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = false;
        validation.validate_aud = false;
        let err = decode::<ParsedDenial>(
            &token,
            &DecodingKey::from_secret(b"signer-b------------------------"),
            &validation,
        );
        assert!(err.is_err());
    }
}
