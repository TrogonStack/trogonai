use std::future::Future;

use reqwest::{StatusCode, Url};
use serde::{Deserialize, Serialize};

use super::constants::KEY_FETCH_TIMEOUT;
use super::verification_key::{PlaidJwk, PlaidVerificationKey, PlaidVerificationKeyError};
use super::{PlaidApiBaseUrl, PlaidClientId, PlaidKeyId, PlaidSecret};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PlaidKeyFetchError {
    #[error("Plaid key request failed")]
    Transport(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("Plaid key service unavailable: status {0}")]
    Unavailable(StatusCode),
    #[error("Plaid does not know the verification key id")]
    UnknownKey,
    #[error("Plaid rejected the key request: status {0}")]
    Rejected(StatusCode),
    #[error("Plaid returned an unusable key")]
    InvalidKey(#[source] PlaidVerificationKeyError),
}

pub trait PlaidKeyFetcher: Clone + Send + Sync + 'static {
    fn fetch(
        &self,
        key_id: &PlaidKeyId,
        secret: &PlaidSecret,
    ) -> impl Future<Output = Result<PlaidVerificationKey, PlaidKeyFetchError>> + Send;
}

#[derive(Clone)]
pub struct ReqwestPlaidKeyFetcher {
    client: reqwest::Client,
    url: Url,
    client_id: PlaidClientId,
}

impl ReqwestPlaidKeyFetcher {
    #[cfg_attr(
        dylint_lib = "trogon_dylint_lints",
        allow(
            fallible_new,
            reason = "the builder only disables redirects over compiled-in rustls roots, so `build` has no runtime failure for a caller to handle"
        )
    )]
    #[allow(clippy::expect_used)]
    pub fn new(api_base_url: &PlaidApiBaseUrl, client_id: PlaidClientId) -> Self {
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("reqwest client with rustls-tls backend must build"),
            url: api_base_url.webhook_verification_key_url().clone(),
            client_id,
        }
    }
}

#[derive(Serialize)]
struct KeyRequest<'a> {
    client_id: &'a str,
    secret: &'a str,
    key_id: &'a str,
}

#[derive(Deserialize)]
struct KeyResponse {
    key: PlaidJwk,
}

#[derive(Deserialize)]
struct ErrorResponse {
    error_code: String,
}

const UNKNOWN_KEY_ERROR_CODE: &str = "INVALID_WEBHOOK_VERIFICATION_KEY_ID";

fn is_transient(status: StatusCode) -> bool {
    status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::REQUEST_TIMEOUT
}

impl PlaidKeyFetcher for ReqwestPlaidKeyFetcher {
    async fn fetch(
        &self,
        key_id: &PlaidKeyId,
        secret: &PlaidSecret,
    ) -> Result<PlaidVerificationKey, PlaidKeyFetchError> {
        let request = KeyRequest {
            client_id: self.client_id.as_str(),
            secret: secret.as_str(),
            key_id: key_id.as_str(),
        };
        let response = self
            .client
            .post(self.url.clone())
            .timeout(KEY_FETCH_TIMEOUT)
            .json(&request)
            .send()
            .await
            .map_err(|error| PlaidKeyFetchError::Transport(Box::new(error)))?;

        let status = response.status();
        if is_transient(status) {
            return Err(PlaidKeyFetchError::Unavailable(status));
        }
        if !status.is_success() {
            return Err(match response.json::<ErrorResponse>().await {
                Ok(body) if body.error_code == UNKNOWN_KEY_ERROR_CODE => PlaidKeyFetchError::UnknownKey,
                _ => PlaidKeyFetchError::Rejected(status),
            });
        }

        let body: KeyResponse = response
            .json()
            .await
            .map_err(|error| PlaidKeyFetchError::Transport(Box::new(error)))?;
        PlaidVerificationKey::from_jwk(&body.key, key_id).map_err(PlaidKeyFetchError::InvalidKey)
    }
}

#[cfg(test)]
mod tests;
