use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use aws_sdk_kms::types::{MessageType, SigningAlgorithmSpec};
use jsonwebtoken::{Algorithm, Header};
use serde_json::Value;

use crate::jwks::Jwks;
use crate::keys::KeyError;
use crate::keys::common::spki_der_to_jwk;
use crate::signer::{DEFAULT_MESH_ISSUER, MeshSigner, assemble_rs256_jwt, rs256_signing_input};

pub struct KmsSigner {
    client: aws_sdk_kms::Client,
    key_id: String,
    previous_key_version: Option<String>,
    issuer: String,
    signing_kid: Arc<str>,
}

impl KmsSigner {
    pub async fn new(
        key_id: impl Into<String>,
        region: Option<String>,
        previous_key_version: Option<String>,
        issuer: Option<String>,
    ) -> Result<Self, KeyError> {
        let key_id = key_id.into();
        let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
        if let Some(region) = region {
            loader = loader.region(aws_config::Region::new(region));
        }
        let config = loader.load().await;
        let client = aws_sdk_kms::Client::new(&config);
        let signer = Self {
            client,
            key_id: key_id.clone(),
            previous_key_version,
            issuer: issuer.unwrap_or_else(|| DEFAULT_MESH_ISSUER.into()),
            signing_kid: Arc::from(key_id),
        };
        let (kid, _) = signer.fetch_public_jwk(&signer.key_id).await?;
        Ok(Self {
            signing_kid: Arc::from(kid),
            ..signer
        })
    }

    pub async fn from_client(
        client: aws_sdk_kms::Client,
        key_id: impl Into<String>,
        previous_key_version: Option<String>,
        issuer: Option<String>,
    ) -> Result<Self, KeyError> {
        let key_id = key_id.into();
        let signer = Self {
            client,
            key_id: key_id.clone(),
            previous_key_version,
            issuer: issuer.unwrap_or_else(|| DEFAULT_MESH_ISSUER.into()),
            signing_kid: Arc::from(key_id),
        };
        let (kid, _) = signer.fetch_public_jwk(&signer.key_id).await?;
        Ok(Self {
            signing_kid: Arc::from(kid),
            ..signer
        })
    }

    async fn fetch_public_jwk(&self, key_id: &str) -> Result<(String, jsonwebtoken::jwk::Jwk), KeyError> {
        let output = self
            .client
            .get_public_key()
            .key_id(key_id)
            .send()
            .await
            .map_err(|e| KeyError::Kms(format!("GetPublicKey: {e}")))?;
        let der = output
            .public_key()
            .ok_or_else(|| KeyError::Kms("GetPublicKey returned empty key material".into()))?;
        let aws_kid = output
            .key_id()
            .map(str::to_owned)
            .unwrap_or_else(|| key_id.to_owned());
        let jwk = spki_der_to_jwk(der.as_ref(), Some(aws_kid.clone()))?;
        Ok((aws_kid, jwk))
    }
}

#[async_trait]
impl MeshSigner for KmsSigner {
    fn current_kid(&self) -> &str {
        &self.signing_kid
    }

    fn mesh_issuer(&self) -> &str {
        &self.issuer
    }

    async fn sign(&self, claims: &HashMap<String, Value>) -> Result<String, KeyError> {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(self.signing_kid.to_string());
        header.typ = Some("JWT".into());
        let signing_input = rs256_signing_input(&header, claims)?;
        let signature = self
            .client
            .sign()
            .key_id(&self.key_id)
            .message(signing_input.into())
            .message_type(MessageType::Raw)
            .signing_algorithm(SigningAlgorithmSpec::RsassaPkcs1V15Sha256)
            .send()
            .await
            .map_err(|e| KeyError::Kms(format!("Sign: {e}")))?
            .signature()
            .ok_or_else(|| KeyError::Kms("Sign returned empty signature".into()))?
            .as_ref()
            .to_vec();
        assemble_rs256_jwt(&header, claims, &signature)
    }

    async fn active_jwks(&self) -> Result<Jwks, KeyError> {
        let mut keys = Vec::new();
        let (_, current_jwk) = self.fetch_public_jwk(&self.key_id).await?;
        keys.push(current_jwk);
        if let Some(version) = &self.previous_key_version {
            let previous_key_id = format!("{}:{}", self.key_id, version);
            if let Ok((_, previous_jwk)) = self.fetch_public_jwk(&previous_key_id).await
                && !keys.iter().any(|jwk| jwk.common.key_id == previous_jwk.common.key_id)
            {
                keys.push(previous_jwk);
            }
        }
        if keys.is_empty() {
            return Err(KeyError::EmptyKeySet);
        }
        Ok(Jwks { keys })
    }
}
