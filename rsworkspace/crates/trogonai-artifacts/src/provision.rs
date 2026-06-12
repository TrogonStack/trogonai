use std::time::Duration;

use async_nats::jetstream;
use trogon_nats::jetstream::NatsObjectStore;

use crate::config::session_artifacts_bucket;
use crate::error::ArtifactStoreError;
use trogonai_session_kernel::SessionKernelConfig;

/// Provisions (or reuses) the NATS Object Store bucket for session artifacts.
pub async fn provision_artifact_object_store(
    js: &jetstream::Context,
    config: &SessionKernelConfig,
) -> Result<NatsObjectStore, ArtifactStoreError> {
    let bucket = session_artifacts_bucket(&config.nats_prefix);
    NatsObjectStore::provision(
        js,
        jetstream::object_store::Config {
            bucket,
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
            ..Default::default()
        },
    )
    .await
    .map_err(|err| ArtifactStoreError::ObjectStorePut {
        session_id: trogonai_session_contracts::SessionId::new("sess_provision")
            .expect("valid session id"),
        detail: err.to_string(),
    })
}
