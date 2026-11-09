use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::DecodingKey;
use serde::Deserialize;

use super::PlaidKeyId;
use super::plaid_key_id::PlaidKeyIdError;

#[derive(Debug, Clone, Deserialize)]
pub struct PlaidJwk {
    pub alg: String,
    pub crv: String,
    pub kty: String,
    pub kid: String,
    pub x: String,
    pub y: String,
    #[serde(default)]
    pub expired_at: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PlaidVerificationKeyError {
    #[error("returned key id is invalid")]
    InvalidKeyId(#[source] PlaidKeyIdError),
    #[error("returned key id {returned} does not match requested key id {requested}")]
    KeyIdMismatch {
        requested: PlaidKeyId,
        returned: PlaidKeyId,
    },
    #[error("unsupported key parameters: alg {alg}, kty {kty}, crv {crv}")]
    UnsupportedParameters { alg: String, kty: String, crv: String },
    #[error("invalid key coordinates")]
    InvalidCoordinates(#[source] jsonwebtoken::errors::Error),
}

#[derive(Clone, Debug)]
pub struct PlaidVerificationKey {
    key_id: PlaidKeyId,
    decoding_key: DecodingKey,
    expired_at: Option<SystemTime>,
}

impl PlaidVerificationKey {
    pub fn from_jwk(jwk: &PlaidJwk, requested: &PlaidKeyId) -> Result<Self, PlaidVerificationKeyError> {
        let key_id = PlaidKeyId::new(&jwk.kid).map_err(PlaidVerificationKeyError::InvalidKeyId)?;
        if &key_id != requested {
            return Err(PlaidVerificationKeyError::KeyIdMismatch {
                requested: requested.clone(),
                returned: key_id,
            });
        }
        if jwk.alg != "ES256" || jwk.kty != "EC" || jwk.crv != "P-256" {
            return Err(PlaidVerificationKeyError::UnsupportedParameters {
                alg: jwk.alg.clone(),
                kty: jwk.kty.clone(),
                crv: jwk.crv.clone(),
            });
        }
        let decoding_key =
            DecodingKey::from_ec_components(&jwk.x, &jwk.y).map_err(PlaidVerificationKeyError::InvalidCoordinates)?;
        Ok(Self {
            key_id,
            decoding_key,
            expired_at: jwk.expired_at.map(|secs| UNIX_EPOCH + Duration::from_secs(secs)),
        })
    }

    pub fn key_id(&self) -> &PlaidKeyId {
        &self.key_id
    }

    pub fn decoding_key(&self) -> &DecodingKey {
        &self.decoding_key
    }

    pub fn is_expired_at(&self, now: SystemTime) -> bool {
        self.expired_at.is_some_and(|expired_at| expired_at <= now)
    }
}

#[cfg(test)]
mod tests;
