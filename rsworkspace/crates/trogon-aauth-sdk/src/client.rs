//! Person Server clients. Two transports, same surface:
//!
//! * [`PersonHttpClient`] — JSON over HTTPS.
//! * [`PersonNatsClient`] — NATS req/rep with `aauth.{ps_id}.bootstrap` and
//!   `aauth.{ps_id}.token`.

use std::time::Duration;

use crate::bootstrap::{BootstrapRequest, BootstrapResponse, TokenRequest, TokenResponse};
use crate::error::SdkError;

/// HTTP client for a Person Server.
#[derive(Clone)]
pub struct PersonHttpClient {
    pub base_url: String,
    pub client: reqwest::Client,
}

impl PersonHttpClient {
    /// Construct with a default reqwest client.
    pub fn new(base_url: impl Into<String>) -> Result<Self, SdkError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self {
            base_url: base_url.into(),
            client,
        })
    }

    /// POST `/aauth/agent`.
    pub async fn bootstrap(&self, req: BootstrapRequest) -> Result<BootstrapResponse, SdkError> {
        let url = format!("{}/aauth/agent", self.base_url.trim_end_matches('/'));
        let resp = self.client.post(&url).json(&req).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SdkError::Server { status: status.as_u16(), body });
        }
        let resp = resp
            .json::<BootstrapResponse>()
            .await
            .map_err(|e| SdkError::InvalidResponse(format!("bootstrap body: {e}")))?;
        Ok(resp)
    }

    /// POST `/aauth/token`.
    pub async fn exchange(&self, req: TokenRequest) -> Result<TokenResponse, SdkError> {
        let url = format!("{}/aauth/token", self.base_url.trim_end_matches('/'));
        let resp = self.client.post(&url).json(&req).send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::ACCEPTED {
            let v: serde_json::Value = resp.json().await.map_err(|e| {
                SdkError::InvalidResponse(format!("interaction body: {e}"))
            })?;
            let url = v["url"].as_str().unwrap_or_default().to_string();
            let code = v["code"].as_str().map(|s| s.to_string());
            return Err(SdkError::Interaction { url, code });
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SdkError::Server { status: status.as_u16(), body });
        }
        let body = resp
            .json::<TokenResponse>()
            .await
            .map_err(|e| SdkError::InvalidResponse(format!("token body: {e}")))?;
        Ok(body)
    }
}

/// NATS client for a Person Server.
#[derive(Clone)]
pub struct PersonNatsClient {
    pub client: async_nats::Client,
    pub ps_id: String,
    pub subject_prefix: String,
    pub request_timeout: Duration,
}

impl PersonNatsClient {
    #[must_use]
    pub fn new(client: async_nats::Client, ps_id: impl Into<String>) -> Self {
        Self {
            client,
            ps_id: ps_id.into(),
            subject_prefix: "aauth".into(),
            request_timeout: Duration::from_secs(10),
        }
    }

    #[must_use]
    pub fn subject(&self, name: &str) -> String {
        format!("{}.{}.{}", self.subject_prefix, self.ps_id, name)
    }

    pub async fn bootstrap(&self, req: BootstrapRequest) -> Result<BootstrapResponse, SdkError> {
        let bytes = serde_json::to_vec(&req)
            .map_err(|e| SdkError::InvalidResponse(format!("encode bootstrap: {e}")))?;
        let msg = tokio::time::timeout(
            self.request_timeout,
            self.client.request(self.subject("bootstrap"), bytes.into()),
        )
        .await
        .map_err(|_| SdkError::Nats("bootstrap timeout".into()))?
        .map_err(|e| SdkError::Nats(e.to_string()))?;
        decode_nats_response(&msg)
    }

    pub async fn exchange(&self, req: TokenRequest) -> Result<TokenResponse, SdkError> {
        let bytes = serde_json::to_vec(&req)
            .map_err(|e| SdkError::InvalidResponse(format!("encode token: {e}")))?;
        let msg = tokio::time::timeout(
            self.request_timeout,
            self.client.request(self.subject("token"), bytes.into()),
        )
        .await
        .map_err(|_| SdkError::Nats("token timeout".into()))?
        .map_err(|e| SdkError::Nats(e.to_string()))?;
        decode_nats_response(&msg)
    }
}

fn decode_nats_response<T: serde::de::DeserializeOwned>(
    msg: &async_nats::Message,
) -> Result<T, SdkError> {
    if let Some(headers) = &msg.headers
        && let Some(code) = headers.get("Nats-Service-Error-Code")
    {
        let status = code.to_string().parse::<u16>().unwrap_or(500);
        let detail = headers
            .get("Nats-Service-Error")
            .map(|v| v.to_string())
            .unwrap_or_default();
        if status == 202 {
            return Err(SdkError::Interaction { url: detail, code: None });
        }
        return Err(SdkError::Server { status, body: detail });
    }
    serde_json::from_slice::<T>(&msg.payload).map_err(|e| {
        SdkError::InvalidResponse(format!("decode response: {e}"))
    })
}
