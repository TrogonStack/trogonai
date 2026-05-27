use std::collections::HashMap;

use async_trait::async_trait;
use aws_sdk_kms::primitives::Blob;
use aws_sdk_kms::types::{MessageType, SigningAlgorithmSpec};
use jsonwebtoken::Algorithm;
use serde_json::Value;

use super::{Signer, assemble_jwt, jwt_signing_input};
use crate::error::StsError;

pub struct KmsSigner {
    client: aws_sdk_kms::Client,
    key_id: String,
    kid: String,
}

impl KmsSigner {
    pub async fn new(key_id: impl Into<String>, kid: impl Into<String>) -> Result<Self, StsError> {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        Ok(Self {
            client: aws_sdk_kms::Client::new(&config),
            key_id: key_id.into(),
            kid: kid.into(),
        })
    }

    pub fn from_client(client: aws_sdk_kms::Client, key_id: impl Into<String>, kid: impl Into<String>) -> Self {
        Self {
            client,
            key_id: key_id.into(),
            kid: kid.into(),
        }
    }
}

#[async_trait]
impl Signer for KmsSigner {
    fn current_kid(&self) -> String {
        self.kid.clone()
    }

    async fn sign(&self, claims: &HashMap<String, Value>) -> Result<String, StsError> {
        let signing_input = jwt_signing_input(Algorithm::RS256, &self.kid, claims)?;
        let output = self
            .client
            .sign()
            .key_id(&self.key_id)
            .message(Blob::new(signing_input.as_bytes()))
            .message_type(MessageType::Raw)
            .signing_algorithm(SigningAlgorithmSpec::RsassaPkcs1V15Sha256)
            .send()
            .await
            .map_err(|e| StsError::SignerUnavailable(format!("kms sign: {e}")))?;
        let signature = output
            .signature()
            .ok_or_else(|| StsError::SignerUnavailable("kms sign returned empty signature".into()))?;
        Ok(assemble_jwt(&signing_input, signature.as_ref()))
    }
}
