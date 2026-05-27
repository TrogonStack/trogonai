use std::sync::Arc;

use async_nats::Client;
use tokio::time::timeout;
use tracing::debug;
use trogon_sts::{StsExchangeRequest, StsExchangeResponse, StsTokenErrorResponse};

use crate::config::StsClientConfig;
use crate::error::StsClientError;

const JWT_TOKEN_TYPE: &str = "urn:ietf:params:oauth:token-type:jwt";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MintedMeshToken {
    pub access_token: String,
    pub expires_in: u64,
    pub exp: i64,
    pub iss: String,
    pub kid: Option<String>,
}

#[derive(Clone)]
pub struct StsClient {
    client: Client,
    config: StsClientConfig,
}

impl StsClient {
    pub fn new(client: Client, config: StsClientConfig) -> Self {
        Self { client, config }
    }

    pub fn from_arc(client: Arc<Client>, config: StsClientConfig) -> Self {
        Self {
            client: (*client).clone(),
            config,
        }
    }

    pub fn config(&self) -> &StsClientConfig {
        &self.config
    }

    pub async fn exchange(&self, request: StsExchangeRequest) -> Result<MintedMeshToken, StsClientError> {
        let payload = serde_json::to_vec(&request).map_err(|e| StsClientError::Decode(e.to_string()))?;
        let subject = self.config.exchange_subject.clone();
        let nats_timeout = self.config.timeout;

        let response = timeout(nats_timeout, self.client.request(subject, payload.into()))
            .await
            .map_err(|_| StsClientError::Timeout(nats_timeout))?
            .map_err(|e| StsClientError::Transport(e.to_string()))?;

        decode_exchange_response(&response.payload)
    }
}

pub fn build_exchange_request(
    subject_token: &str,
    actor_token: &str,
    audience: &str,
    scope: &str,
    purpose: &str,
) -> StsExchangeRequest {
    StsExchangeRequest {
        subject_token: subject_token.to_string(),
        subject_token_type: JWT_TOKEN_TYPE.to_string(),
        actor_token: actor_token.to_string(),
        audience: audience.to_string(),
        scope: scope.to_string(),
        purpose: purpose.to_string(),
        requested_token_type: JWT_TOKEN_TYPE.to_string(),
    }
}

fn decode_exchange_response(body: &[u8]) -> Result<MintedMeshToken, StsClientError> {
    if let Ok(err) = serde_json::from_slice::<StsTokenErrorResponse>(body)
        && !err.error.is_empty()
    {
        return Err(StsClientError::from_wire_error(&err));
    }

    let success: StsExchangeResponse =
        serde_json::from_slice(body).map_err(|e| StsClientError::Decode(e.to_string()))?;

    let (exp, iss, kid) = decode_jwt_claims(&success.access_token)?;
    debug!(
        event = "sts_exchange_ok",
        iss = %iss,
        exp,
        kid = kid.as_deref().unwrap_or(""),
        expires_in = success.expires_in,
        "STS minted mesh token"
    );

    Ok(MintedMeshToken {
        access_token: success.access_token,
        expires_in: success.expires_in,
        exp,
        iss,
        kid,
    })
}

fn decode_jwt_claims(token: &str) -> Result<(i64, String, Option<String>), StsClientError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return Err(StsClientError::Decode("malformed jwt".into()));
    }
    let header: serde_json::Value =
        serde_json::from_slice(&base64_decode_url(parts[0])?).map_err(|e| StsClientError::Decode(e.to_string()))?;
    let kid = header.get("kid").and_then(|v| v.as_str()).map(str::to_string);
    let payload = base64_decode_url(parts[1])?;
    let value: serde_json::Value =
        serde_json::from_slice(&payload).map_err(|e| StsClientError::Decode(e.to_string()))?;
    let exp = value
        .get("exp")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| StsClientError::Decode("missing exp".into()))?;
    let iss = value
        .get("iss")
        .and_then(|v| v.as_str())
        .ok_or_else(|| StsClientError::Decode("missing iss".into()))?
        .to_string();
    Ok((exp, iss, kid))
}

fn base64_decode_url(input: &str) -> Result<Vec<u8>, StsClientError> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input)
        .map_err(|e| StsClientError::Decode(format!("base64 payload: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request_sets_rfc8693_types() {
        let req = build_exchange_request("subj", "actor", "aud", "scope", "purpose");
        assert_eq!(req.subject_token_type, JWT_TOKEN_TYPE);
        assert_eq!(req.requested_token_type, JWT_TOKEN_TYPE);
        assert_eq!(req.audience, "aud");
    }

    #[test]
    fn rejects_sts_error_envelope() {
        let body = br#"{"error":"invalid_target","error_description":"bad aud"}"#;
        let err = decode_exchange_response(body).unwrap_err();
        assert!(matches!(err, StsClientError::ExchangeRejected { .. }));
    }
}
