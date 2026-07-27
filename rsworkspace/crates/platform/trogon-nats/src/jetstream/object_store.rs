use std::error::Error;
use std::future::Future;

use tokio::io::AsyncRead;

#[cfg(not(coverage))]
use async_nats::jetstream::context::CreateKeyValueErrorKind;

pub trait ObjectStorePut: Send + Sync + Clone + 'static {
    type Error: Error + Send + Sync;
    type Info: Send;

    fn put<R: AsyncRead + Unpin + Send>(
        &self,
        name: &str,
        data: &mut R,
    ) -> impl Future<Output = Result<Self::Info, Self::Error>> + Send;
}

pub trait ObjectStoreGet: Send + Sync + Clone + 'static {
    type Error: Error + Send + Sync;
    type Reader: AsyncRead + Unpin + Send;

    fn get(&self, name: &str) -> impl Future<Output = Result<Self::Reader, Self::Error>> + Send;
}

#[cfg(not(coverage))]
#[derive(Debug, thiserror::Error)]
pub enum ProvisionObjectStoreError {
    #[error("failed to create object store: {0}")]
    Create(#[source] async_nats::jetstream::context::CreateObjectStoreError),
    #[error("failed to get existing object store: {0}")]
    Get(#[source] async_nats::jetstream::context::ObjectStoreError),
    #[error("failed to read backing stream for bucket: {0}")]
    GetStream(#[source] async_nats::jetstream::context::GetStreamError),
    #[error("failed to reconcile bucket retention: {0}")]
    UpdateStream(#[source] async_nats::jetstream::context::UpdateStreamError),
}

#[cfg(not(coverage))]
#[derive(Clone)]
pub struct NatsObjectStore {
    store: async_nats::jetstream::object_store::ObjectStore,
}

#[cfg(not(coverage))]
impl NatsObjectStore {
    pub async fn provision(
        js: &async_nats::jetstream::Context,
        config: async_nats::jetstream::object_store::Config,
    ) -> Result<Self, ProvisionObjectStoreError> {
        let bucket = config.bucket.clone();
        match js.create_object_store(config).await {
            Ok(store) => Ok(Self { store }),
            Err(err) if err.kind() == CreateKeyValueErrorKind::BucketCreate => {
                let store = js
                    .get_object_store(&bucket)
                    .await
                    .map_err(ProvisionObjectStoreError::Get)?;
                Ok(Self { store })
            }
            Err(err) => Err(ProvisionObjectStoreError::Create(err)),
        }
    }

    /// Provision a bucket that backs claim-check payloads, sizing its `max_age`
    /// from [`ClaimRetention`] so the object always outlives the messages that
    /// reference it. Callers cannot forget the retention or let it drift from
    /// the owning stream.
    ///
    /// [`provision`](Self::provision) fetches an existing bucket and drops the
    /// requested config, so a bucket created by an earlier deploy would keep its
    /// old `max_age`. This reconciles the backing stream's retention on every
    /// startup, so raising stream retention takes effect instead of silently
    /// expiring resolvable claims.
    pub async fn provision_claim_bucket(
        js: &async_nats::jetstream::Context,
        bucket: impl Into<String>,
        retention: super::claim_retention::ClaimRetention,
    ) -> Result<Self, ProvisionObjectStoreError> {
        let bucket = bucket.into();
        let max_age = retention.bucket_max_age();
        let store = Self::provision(
            js,
            async_nats::jetstream::object_store::Config {
                bucket: bucket.clone(),
                max_age,
                ..Default::default()
            },
        )
        .await?;
        reconcile_bucket_max_age(js, &bucket, max_age).await?;
        Ok(store)
    }
}

/// Bring the retention of an already-provisioned bucket in line with `max_age`.
/// A NATS object-store bucket is a stream named `OBJ_<bucket>`; only `max_age`
/// is touched, leaving every other stream setting untouched.
#[cfg(not(coverage))]
async fn reconcile_bucket_max_age(
    js: &async_nats::jetstream::Context,
    bucket: &str,
    max_age: std::time::Duration,
) -> Result<(), ProvisionObjectStoreError> {
    let stream = js
        .get_stream(format!("OBJ_{bucket}"))
        .await
        .map_err(ProvisionObjectStoreError::GetStream)?;
    let mut config = stream.cached_info().config.clone();
    if config.max_age == max_age {
        return Ok(());
    }
    config.max_age = max_age;
    js.update_stream(&config)
        .await
        .map_err(ProvisionObjectStoreError::UpdateStream)?;
    Ok(())
}

#[cfg(not(coverage))]
impl ObjectStorePut for NatsObjectStore {
    type Error = async_nats::jetstream::object_store::PutError;
    type Info = async_nats::jetstream::object_store::ObjectInfo;

    async fn put<R: AsyncRead + Unpin + Send>(&self, name: &str, data: &mut R) -> Result<Self::Info, Self::Error> {
        self.store.put(name, data).await
    }
}

#[cfg(not(coverage))]
impl ObjectStoreGet for NatsObjectStore {
    type Error = async_nats::jetstream::object_store::GetError;
    type Reader = async_nats::jetstream::object_store::Object;

    async fn get(&self, name: &str) -> Result<Self::Reader, Self::Error> {
        self.store.get(name).await
    }
}

#[cfg(all(test, not(coverage), feature = "test-support"))]
mod integration_tests;
