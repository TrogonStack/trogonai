use async_trait::async_trait;
use jsonwebtoken::DecodingKey;

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
