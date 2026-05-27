use async_trait::async_trait;
use jsonwebtoken::DecodingKey;
use jsonwebtoken::jwk::JwkSet;

use crate::traits::Jwks;
use crate::types::SdkError;

/// Dev/test JWKS source using a static HS256 secret.
pub struct Hs256Jwks {
    secret: Vec<u8>,
}

impl Hs256Jwks {
    pub fn new(secret: impl Into<Vec<u8>>) -> Self {
        Self { secret: secret.into() }
    }
}

#[async_trait]
impl Jwks for Hs256Jwks {
    async fn decoding_key(&self, _kid: Option<&str>) -> Result<DecodingKey, SdkError> {
        Ok(DecodingKey::from_secret(&self.secret))
    }
}

/// Mesh JWT verification using an RS256 JWKS set (production / integration tests).
pub struct Rs256Jwks {
    jwks: JwkSet,
}

impl Rs256Jwks {
    pub fn new(jwks: JwkSet) -> Self {
        Self { jwks }
    }
}

#[async_trait]
impl Jwks for Rs256Jwks {
    async fn decoding_key(&self, kid: Option<&str>) -> Result<DecodingKey, SdkError> {
        let key = if let Some(kid) = kid {
            self.jwks.find(kid).ok_or_else(|| SdkError::InvalidToken(format!("unknown kid {kid}")))?
        } else {
            self.jwks
                .keys
                .first()
                .ok_or_else(|| SdkError::InvalidToken("empty JWKS".into()))?
        };
        DecodingKey::from_jwk(key).map_err(|e| SdkError::InvalidToken(format!("jwk decode: {e}")))
    }
}
