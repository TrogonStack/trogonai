use async_nats::Client;
use async_trait::async_trait;
use serde::Deserialize;

use crate::constants::STS_EXCHANGE_SUBJECT;
use crate::traits::Sts;
use crate::types::{ExchangeRequest, ExchangeResponse, SdkError};

pub struct NatsSts {
    client: Client,
    subject: String,
}

impl NatsSts {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            subject: STS_EXCHANGE_SUBJECT.to_owned(),
        }
    }

    pub fn with_subject(client: Client, subject: impl Into<String>) -> Self {
        Self {
            client,
            subject: subject.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct StsErrorEnvelope {
    error: Option<String>,
    error_description: Option<String>,
}

#[async_trait]
impl Sts for NatsSts {
    async fn exchange(&self, req: ExchangeRequest) -> Result<ExchangeResponse, SdkError> {
        let payload = serde_json::to_vec(&req).map_err(|e| SdkError::Serialization(e.to_string()))?;
        let response = self
            .client
            .request(self.subject.clone(), payload.into())
            .await
            .map_err(SdkError::nats)?;
        let body = response.payload;
        if let Ok(err) = serde_json::from_slice::<StsErrorEnvelope>(&body)
            && (err.error.is_some() || err.error_description.is_some())
        {
            let msg = err
                .error_description
                .or(err.error)
                .unwrap_or_else(|| "STS exchange rejected".to_owned());
            return Err(SdkError::ExchangeFailed(msg));
        }
        serde_json::from_slice(&body).map_err(|e| SdkError::ExchangeFailed(e.to_string()))
    }
}
