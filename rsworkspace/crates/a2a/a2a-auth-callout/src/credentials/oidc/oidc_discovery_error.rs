use serde::de::DeserializeOwned;

use crate::error::{AuthCalloutError, CredentialError};

const MAX_DOCUMENT_BYTES: usize = 256 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum OidcDiscoveryError {
    #[error("OIDC discovery transport failed")]
    Http(#[source] reqwest::Error),
    #[error("OIDC discovery returned an unsuccessful HTTP status: {0}")]
    Status(reqwest::StatusCode),
    #[error("OIDC discovery document is invalid")]
    Json(#[source] serde_json::Error),
    #[error("OIDC discovery document exceeds the size limit")]
    TooLarge,
    #[error("OIDC discovery issuer does not exactly match the configured identity")]
    IssuerMismatch,
    #[error("OIDC JWKS location must be a credential-free URL on the issuer origin")]
    JwksLocation,
}

pub(super) async fn fetch_document<T: DeserializeOwned>(
    http: &reqwest::Client,
    uri: &str,
) -> Result<T, OidcDiscoveryError> {
    let mut response = http.get(uri).send().await.map_err(OidcDiscoveryError::Http)?;
    if !response.status().is_success() {
        return Err(OidcDiscoveryError::Status(response.status()));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_DOCUMENT_BYTES as u64)
    {
        return Err(OidcDiscoveryError::TooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(OidcDiscoveryError::Http)? {
        if bytes.len().saturating_add(chunk.len()) > MAX_DOCUMENT_BYTES {
            return Err(OidcDiscoveryError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(OidcDiscoveryError::Json)
}

impl From<OidcDiscoveryError> for AuthCalloutError {
    fn from(error: OidcDiscoveryError) -> Self {
        CredentialError::OidcDiscovery(error).into()
    }
}
