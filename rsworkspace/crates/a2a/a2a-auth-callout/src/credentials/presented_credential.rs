use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::api_key::{ApiKey, ApiKeyError};
use super::mtls::ClientCertPem;
use super::oidc::BearerToken;

/// The one credential the callout verifies for a caller.
#[derive(Debug, Clone)]
pub enum PresentedCredential {
    BearerToken(BearerToken),
    ApiKey(ApiKey),
    ClientCertificate(ClientCertPem),
}

impl PresentedCredential {
    /// A secret travels in `auth_token` or `pass` whatever its scheme, so its
    /// shape decides: a compact JWS is a bearer token, anything else an API key.
    pub fn from_secret(secret: &str) -> Result<Self, ApiKeyError> {
        if is_compact_jws(secret) {
            return Ok(Self::BearerToken(BearerToken::new(secret)));
        }
        ApiKey::new(secret).map(Self::ApiKey)
    }
}

fn is_compact_jws(secret: &str) -> bool {
    let mut segments = secret.split('.');
    let (Some(header), Some(_), Some(_), None) = (segments.next(), segments.next(), segments.next(), segments.next())
    else {
        return false;
    };
    URL_SAFE_NO_PAD
        .decode(header)
        .ok()
        .and_then(|header| serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&header).ok())
        .is_some_and(|header| header.contains_key("alg"))
}

#[cfg(test)]
mod tests;
