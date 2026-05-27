use std::fmt;

use async_nats::jetstream::{Context as JetStreamContext, kv};
use tokio::sync::watch;
use tracing::{info, warn};

use crate::jwks::{Jwks, KV_BUCKET, KV_KEY_CURRENT};

pub const ENV_CREATE_KV_BUCKET: &str = "TROGON_JWKS_KV_CREATE_BUCKET";

#[derive(Debug)]
pub enum KvPublishError {
    Connect(async_nats::ConnectError),
    OpenBucket(async_nats::jetstream::context::KeyValueError),
    Put(String),
    Serialize(serde_json::Error),
}

impl fmt::Display for KvPublishError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(err) => write!(f, "nats connect failed: {err}"),
            Self::OpenBucket(err) => write!(f, "open kv bucket failed: {err}"),
            Self::Put(err) => write!(f, "kv put failed: {err}"),
            Self::Serialize(err) => write!(f, "jwks serialize failed: {err}"),
        }
    }
}

impl std::error::Error for KvPublishError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Connect(err) => Some(err),
            Self::OpenBucket(err) => Some(err),
            Self::Put(_) => None,
            Self::Serialize(err) => Some(err),
        }
    }
}

pub fn serialize_jwks(jwks: &Jwks) -> Result<Vec<u8>, KvPublishError> {
    jwks.to_json().map_err(KvPublishError::Serialize)
}

pub fn kid_kv_key(kid: &str) -> String {
    format!("mesh/{kid}")
}

pub async fn open_kv_store(js: &JetStreamContext, create_if_missing: bool) -> Result<kv::Store, KvPublishError> {
    if create_if_missing {
        let config = kv::Config {
            bucket: KV_BUCKET.to_owned(),
            ..Default::default()
        };
        if let Ok(store) = js.create_key_value(config).await {
            return Ok(store);
        }
    }

    js.get_key_value(KV_BUCKET.to_owned())
        .await
        .map_err(KvPublishError::OpenBucket)
}

pub async fn publish_jwks_to_kv(store: &kv::Store, jwks: &Jwks) -> Result<(), KvPublishError> {
    let payload = serialize_jwks(jwks)?;
    store
        .put(KV_KEY_CURRENT, payload.clone().into())
        .await
        .map_err(|err| KvPublishError::Put(err.to_string()))?;
    for kid in jwks.kids() {
        store
            .put(kid_kv_key(kid), payload.clone().into())
            .await
            .map_err(|err| KvPublishError::Put(err.to_string()))?;
    }
    info!(keys = jwks.keys.len(), "published JWKS to NATS KV");
    Ok(())
}

pub async fn run_kv_publisher(
    nats_url: &str,
    mut watch_rx: watch::Receiver<Jwks>,
    create_if_missing: bool,
) -> Result<(), KvPublishError> {
    let client = async_nats::connect(nats_url).await.map_err(KvPublishError::Connect)?;
    let js = async_nats::jetstream::new(client);
    let store = open_kv_store(&js, create_if_missing).await?;

    loop {
        let jwks = watch_rx.borrow().clone();
        if let Err(err) = publish_jwks_to_kv(&store, &jwks).await {
            warn!(error = %err, "failed to publish JWKS to NATS KV");
        }

        if watch_rx.changed().await.is_err() {
            break;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use jsonwebtoken::jwk::{
        AlgorithmParameters, CommonParameters, Jwk, KeyOperations, PublicKeyUse, RSAKeyParameters, RSAKeyType,
    };
    use rand::rngs::OsRng;
    use rsa::RsaPrivateKey;
    use rsa::traits::PublicKeyParts;

    use super::*;

    fn b64url_uint_be(bytes: &[u8]) -> String {
        let start = bytes
            .iter()
            .position(|&b| b != 0)
            .unwrap_or(bytes.len().saturating_sub(1));
        let trimmed = if start >= bytes.len() {
            &bytes[bytes.len().saturating_sub(1)..]
        } else {
            &bytes[start..]
        };
        URL_SAFE_NO_PAD.encode(trimmed)
    }

    fn sample_jwks() -> Jwks {
        let key = RsaPrivateKey::new(&mut OsRng, 2048).expect("rsa key");
        let public = key.to_public_key();
        let jwk = Jwk {
            common: CommonParameters {
                public_key_use: Some(PublicKeyUse::Signature),
                key_operations: Some(vec![KeyOperations::Verify]),
                key_id: Some("kv-test-kid".into()),
                ..Default::default()
            },
            algorithm: AlgorithmParameters::RSA(RSAKeyParameters {
                key_type: RSAKeyType::RSA,
                n: b64url_uint_be(&public.n().to_bytes_be()),
                e: b64url_uint_be(&public.e().to_bytes_be()),
            }),
        };
        Jwks { keys: vec![jwk] }
    }

    #[test]
    fn serializes_jwks_round_trip_identical_to_rfc7517_shape() {
        let jwks = sample_jwks();
        let bytes = serialize_jwks(&jwks).expect("serialize");
        let parsed: Jwks = serde_json::from_slice(&bytes).expect("parse");
        assert_eq!(parsed, jwks);
        let value = serde_json::from_slice::<serde_json::Value>(&bytes).expect("value");
        assert!(value.get("keys").and_then(|v| v.as_array()).is_some());
    }

    #[test]
    fn kid_kv_key_uses_mesh_prefix() {
        assert_eq!(kid_kv_key("abc123"), "mesh/abc123");
    }
}
