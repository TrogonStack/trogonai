use std::collections::HashMap;

use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use jsonwebtoken::{Algorithm, Header};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde::Deserialize;
use serde_json::Value;

use crate::jwks::Jwks;
use crate::keys::KeyError;
use rsa::pkcs8::{DecodePublicKey, EncodePublicKey};
use rsa::RsaPublicKey;

use crate::keys::common::{kid_from_public_der, rsa_public_der_to_jwk};
use crate::signer::{DEFAULT_MESH_ISSUER, MeshSigner, assemble_rs256_jwt, rs256_signing_input};

pub struct VaultSigner {
    http: reqwest::Client,
    vault_addr: String,
    mount: String,
    key_name: String,
    token: String,
    issuer: String,
    signing_kid: std::sync::Arc<str>,
}

impl VaultSigner {
    pub fn new(
        vault_addr: impl Into<String>,
        mount: impl Into<String>,
        key_name: impl Into<String>,
        token: impl Into<String>,
        issuer: Option<String>,
    ) -> Result<Self, KeyError> {
        Ok(Self {
            http: reqwest::Client::new(),
            vault_addr: vault_addr.into().trim_end_matches('/').to_owned(),
            mount: mount.into(),
            key_name: key_name.into(),
            token: token.into(),
            issuer: issuer.unwrap_or_else(|| DEFAULT_MESH_ISSUER.into()),
            signing_kid: std::sync::Arc::from("vault-uninitialized"),
        })
    }

    pub async fn bootstrap(mut self) -> Result<Self, KeyError> {
        let metadata = self.read_key_metadata().await?;
        let version = metadata.data.latest_version;
        let version_entry = metadata
            .data
            .keys
            .get(&version.to_string())
            .ok_or_else(|| KeyError::Vault(format!("missing vault key version {version}")))?;
        let kid = Self::versioned_public_jwk(version, &version_entry.public_key)?
            .common
            .key_id
            .clone()
            .ok_or_else(|| KeyError::Vault("vault kid missing".into()))?;
        self.signing_kid = std::sync::Arc::from(kid);
        Ok(self)
    }

    fn auth_headers(&self) -> Result<HeaderMap, KeyError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.token))
                .map_err(|e| KeyError::Vault(format!("vault token header: {e}")))?,
        );
        Ok(headers)
    }

    fn sign_path(&self) -> String {
        format!("{}/v1/{}/sign/{}", self.vault_addr, self.mount, self.key_name)
    }

    fn keys_path(&self) -> String {
        format!("{}/v1/{}/keys/{}", self.vault_addr, self.mount, self.key_name)
    }

    async fn read_key_metadata(&self) -> Result<VaultKeyResponse, KeyError> {
        let response = self
            .http
            .get(self.keys_path())
            .headers(self.auth_headers()?)
            .send()
            .await
            .map_err(|e| KeyError::Vault(format!("transit keys request: {e}")))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| KeyError::Vault(format!("transit keys body: {e}")))?;
        if !status.is_success() {
            return Err(KeyError::Vault(format!("transit keys HTTP {status}: {body}")));
        }
        serde_json::from_str(&body).map_err(|e| KeyError::Vault(format!("transit keys json: {e}")))
    }

    pub fn vault_sign_request(input: &[u8]) -> serde_json::Value {
        serde_json::json!({
            "input": STANDARD.encode(input),
        })
    }

    pub fn parse_vault_signature(signature: &str) -> Result<Vec<u8>, KeyError> {
        let payload = signature
            .rsplit(':')
            .next()
            .ok_or_else(|| KeyError::Vault(format!("unexpected vault signature format: {signature}")))?;
        STANDARD
            .decode(payload)
            .map_err(|e| KeyError::Vault(format!("vault signature base64: {e}")))
    }

    fn versioned_public_jwk(version: u64, public_key_pem: &str) -> Result<jsonwebtoken::jwk::Jwk, KeyError> {
        let public = RsaPublicKey::from_public_key_pem(public_key_pem)
            .map_err(|e| KeyError::Vault(format!("vault public key pem: {e}")))?;
        let der = public
            .to_public_key_der()
            .map_err(|e| KeyError::Vault(format!("vault public key der: {e}")))?;
        let kid = format!("vault-{}-v{version}", kid_from_public_der(der.as_bytes()));
        rsa_public_der_to_jwk(der.as_bytes(), Some(kid))
    }
}

#[derive(Debug, Deserialize)]
struct VaultKeyResponse {
    data: VaultKeyData,
}

#[derive(Debug, Deserialize)]
struct VaultKeyData {
    latest_version: u64,
    min_decryption_version: u64,
    keys: HashMap<String, VaultKeyVersion>,
}

#[derive(Debug, Deserialize)]
struct VaultKeyVersion {
    public_key: String,
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
impl MeshSigner for VaultSigner {
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
        let body = Self::vault_sign_request(&signing_input);
        let response = self
            .http
            .post(self.sign_path())
            .headers(self.auth_headers()?)
            .json(&body)
            .send()
            .await
            .map_err(|e| KeyError::Vault(format!("transit sign request: {e}")))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| KeyError::Vault(format!("transit sign body: {e}")))?;
        if !status.is_success() {
            return Err(KeyError::Vault(format!("transit sign HTTP {status}: {text}")));
        }
        let parsed: VaultSignResponse =
            serde_json::from_str(&text).map_err(|e| KeyError::Vault(format!("transit sign json: {e}")))?;
        let signature = Self::parse_vault_signature(&parsed.data.signature)?;
        assemble_rs256_jwt(&header, claims, &signature)
    }

    async fn active_jwks(&self) -> Result<Jwks, KeyError> {
        let metadata = self.read_key_metadata().await?;
        let min = metadata.data.min_decryption_version;
        let max = metadata.data.latest_version;
        let mut keys = Vec::new();
        for version in min..=max {
            let Some(version_entry) = metadata.data.keys.get(&version.to_string()) else {
                continue;
            };
            keys.push(Self::versioned_public_jwk(version, &version_entry.public_key)?);
        }
        if keys.is_empty() {
            return Err(KeyError::EmptyKeySet);
        }
        Ok(Jwks { keys })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_sign_request_base64_encodes_input() {
        let body = VaultSigner::vault_sign_request(b"header.payload");
        assert_eq!(body["input"], "aGVhZGVyLnBheWxvYWQ=");
    }

    #[test]
    fn parse_vault_signature_strips_prefix() {
        let sig = VaultSigner::parse_vault_signature("vault:v1:YWJj").expect("parse");
        assert_eq!(sig, b"abc");
    }
}
