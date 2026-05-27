use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::pkcs8::DecodePrivateKey;
use rsa::RsaPrivateKey;
use serde_json::Value;

use crate::jwks::Jwks;
use crate::keys::KeyError;
use crate::keys::file::{load_jwks_from_dir, parse_private_pem};
use crate::signer::{DEFAULT_MESH_ISSUER, MeshSigner};

pub struct FileMeshSigner {
    kid: String,
    encoding_key: EncodingKey,
    algorithm: Algorithm,
    jwks: Jwks,
    issuer: String,
}

impl FileMeshSigner {
    pub fn from_key_dir(key_dir: &Path, issuer: Option<String>) -> Result<Self, KeyError> {
        let signing_path = resolve_signing_pem(key_dir)?;
        let pem_bytes = std::fs::read(&signing_path).map_err(KeyError::Io)?;
        let jwks = load_jwks_from_dir(key_dir)?;
        let parsed = parse_private_pem(&pem_bytes, &signing_path)?;
        let kid = parsed
            .first()
            .and_then(|jwk| jwk.common.key_id.clone())
            .ok_or(KeyError::EmptyKeySet)?;
        let pem_text = std::str::from_utf8(&pem_bytes)
            .map_err(|e| KeyError::Parse(format!("{} is not valid UTF-8: {e}", signing_path.display())))?;
        let (encoding_key, algorithm) = if RsaPrivateKey::from_pkcs8_pem(pem_text).is_ok() {
            (
                EncodingKey::from_rsa_pem(&pem_bytes)
                    .map_err(|e| KeyError::Parse(format!("rsa signing key: {e}")))?,
                Algorithm::RS256,
            )
        } else {
            (
                EncodingKey::from_ed_pem(&pem_bytes)
                    .map_err(|e| KeyError::Parse(format!("ed25519 signing key: {e}")))?,
                Algorithm::EdDSA,
            )
        };
        Ok(Self {
            kid,
            encoding_key,
            algorithm,
            jwks,
            issuer: issuer.unwrap_or_else(|| DEFAULT_MESH_ISSUER.into()),
        })
    }
}

fn resolve_signing_pem(key_dir: &Path) -> Result<PathBuf, KeyError> {
    let current = key_dir.join("current.pem");
    if current.is_file() {
        return Ok(current);
    }
    let mut paths: Vec<PathBuf> = std::fs::read_dir(key_dir)
        .map_err(KeyError::Io)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("pem"))
        .collect();
    paths.sort();
    paths.into_iter().next().ok_or(KeyError::EmptyKeySet)
}

#[async_trait]
impl MeshSigner for FileMeshSigner {
    fn current_kid(&self) -> &str {
        &self.kid
    }

    fn mesh_issuer(&self) -> &str {
        &self.issuer
    }

    async fn sign(&self, claims: &HashMap<String, Value>) -> Result<String, KeyError> {
        let mut header = Header::new(self.algorithm);
        header.kid = Some(self.kid.clone());
        header.typ = Some("JWT".into());
        encode(&header, claims, &self.encoding_key).map_err(|e| KeyError::Sign(format!("sign mesh token: {e}")))
    }

    async fn active_jwks(&self) -> Result<Jwks, KeyError> {
        Ok(self.jwks.clone())
    }
}
