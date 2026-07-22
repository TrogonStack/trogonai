//! Durable provider/model certification matrix store.
//!
//! `cambio-modelo.md` requires `SwitchSafe` / `Production` certification to come from
//! probes/contract tests, not from a static baseline. This store persists those probe
//! promotions in NATS KV so runner/model certification survives process restarts and can
//! be used by the Switch Safety Gate during canonical switching.

use buffa::Message as _;
use bytes::Bytes;
use time::OffsetDateTime;
use trogon_nats::jetstream::{
    JetStreamCreateKeyValue, JetStreamGetKeyValue, JetStreamKeyValueStatus, JetStreamKeyValueUpdate, JetStreamKvCreate,
    JetStreamKvEntry, JetStreamKvGet,
};
use trogonai_session_contracts::ProviderCertificationMatrix as ProtoProviderCertificationMatrix;

use crate::certification::ProviderCertificationMatrix;
use crate::error::CapabilityError;

const CERTIFICATION_MATRIX_KEY: &str = "capabilities.certification_matrix";

pub fn certification_matrix_bucket(prefix: &str) -> String {
    format!("{prefix}_CAPABILITY_CERTIFICATIONS")
}

#[derive(Clone)]
pub struct CertificationStore<S> {
    store: S,
    prefix: String,
}

impl<S> CertificationStore<S> {
    pub fn new(store: S, prefix: impl Into<String>) -> Self {
        Self {
            store,
            prefix: prefix.into(),
        }
    }

    pub fn bucket_name(&self) -> String {
        certification_matrix_bucket(&self.prefix)
    }
}

impl<S> CertificationStore<S>
where
    S: JetStreamKvGet + JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate + Clone + Send + Sync + 'static,
{
    pub async fn load_matrix(&self) -> Result<Option<ProviderCertificationMatrix>, CapabilityError> {
        let Some(bytes) = self
            .store
            .get(CERTIFICATION_MATRIX_KEY.to_string())
            .await
            .map_err(|err| CapabilityError::CertificationLoad(err.to_string()))?
        else {
            return Ok(None);
        };
        let proto = ProtoProviderCertificationMatrix::decode_from_slice(&bytes)
            .map_err(|err| CapabilityError::CertificationDecode(err.to_string()))?;
        Ok(Some(ProviderCertificationMatrix::from_proto(&proto)))
    }

    pub async fn save_matrix(&self, matrix: &ProviderCertificationMatrix) -> Result<(), CapabilityError> {
        let proto = matrix.to_proto(OffsetDateTime::now_utc());
        let bytes = Bytes::from(proto.encode_to_vec());
        if let Some(entry) = self
            .store
            .entry(CERTIFICATION_MATRIX_KEY.to_string())
            .await
            .map_err(|err| CapabilityError::CertificationStore(err.to_string()))?
        {
            self.store
                .update(CERTIFICATION_MATRIX_KEY, bytes, entry.revision)
                .await
                .map_err(|err| CapabilityError::CertificationStore(err.to_string()))?;
            return Ok(());
        }
        self.store
            .create(CERTIFICATION_MATRIX_KEY, bytes)
            .await
            .map_err(|err| CapabilityError::CertificationStore(err.to_string()))?;
        Ok(())
    }
}

pub async fn provision_certification_store<J, S>(js: &J, prefix: &str) -> Result<S, CapabilityError>
where
    J: JetStreamGetKeyValue<Store = S> + JetStreamCreateKeyValue<Store = S>,
    S: JetStreamKeyValueStatus,
{
    let bucket = certification_matrix_bucket(prefix);
    match js.get_key_value(bucket.clone()).await {
        Ok(store) => Ok(store),
        Err(_) => js
            .create_key_value(async_nats::jetstream::kv::Config {
                bucket,
                ..Default::default()
            })
            .await
            .map_err(|err| CapabilityError::CertificationStore(err.to_string())),
    }
}
