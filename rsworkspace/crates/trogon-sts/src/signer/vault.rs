use std::collections::HashMap;

use async_trait::async_trait;
use base64::Engine;
use jsonwebtoken::Algorithm;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use serde::Deserialize;
use serde_json::Value;

use super::{Signer, assemble_jwt, jwt_signing_input};
use crate::error::StsError;

pub struct VaultSigner {
    http: reqwest::Client,
    addr: String,
    token: String,
    transit_key: String,
    kid: String,
}

impl VaultSigner {
    pub fn new(
        addr: impl Into<String>,
        token: impl Into<String>,
        transit_key: impl Into<String>,
        kid: impl Into<String>,
    ) -> Result<Self, StsError> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| StsError::ServerError(format!("vault http client: {e}")))?;
        Ok(Self {
            http,
            addr: addr.into().trim_end_matches('/').to_string(),
            token: token.into(),
            transit_key: transit_key.into(),
            kid: kid.into(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct VaultSignResponse {
    data: VaultSignData,
}

#[derive(Debug, Deserialize)]
struct VaultSignData {
    signature: String,
}

#[async_trait]
impl Signer for VaultSigner {
    fn current_kid(&self) -> String {
        self.kid.clone()
    }

    async fn sign(&self, claims: &HashMap<String, Value>) -> Result<String, StsError> {
        let signing_input = jwt_signing_input(Algorithm::RS256, &self.kid, claims)?;
        let input = base64::engine::general_purpose::STANDARD.encode(signing_input.as_bytes());
        let url = format!("{}/v1/transit/sign/{}", self.addr, self.transit_key);
        let body = serde_json::json!({
            "input": input,
            "prehashed": false,
            "signature_algorithm": "pkcs1v15"
        });
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Vault-Token",
            HeaderValue::from_str(&self.token)
                .map_err(|e| StsError::ServerError(format!("vault token header: {e}")))?,
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let response = self
            .http
            .post(&url)
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(|e| StsError::SignerUnavailable(format!("vault sign request: {e}")))?
            .error_for_status()
            .map_err(|e| StsError::SignerUnavailable(format!("vault sign HTTP: {e}")))?;
        let parsed: VaultSignResponse = response
            .json()
            .await
            .map_err(|e| StsError::SignerUnavailable(format!("vault sign json: {e}")))?;
        let signature = parse_vault_signature(&parsed.data.signature)?;
        Ok(assemble_jwt(&signing_input, &signature))
    }
}

fn parse_vault_signature(raw: &str) -> Result<Vec<u8>, StsError> {
    let payload = raw
        .split(':')
        .next_back()
        .ok_or_else(|| StsError::SignerUnavailable(format!("vault signature malformed: {raw}")))?;
    base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|e| StsError::SignerUnavailable(format!("vault signature base64: {e}")))
}
