use std::error::Error;
use std::future::Future;
use std::time::Duration;

use tokio::io::AsyncRead;

#[cfg(not(coverage))]
use async_nats::jetstream::context::CreateKeyValueErrorKind;

/// Decide the `max_age` a claim bucket should reconcile to, growing only.
///
/// A claim object must outlive every message that can still reference it, and
/// source streams themselves never shrink retention (`get_or_create_stream`).
/// Shrinking the bucket would expire live claims, so the bucket only ever
/// widens: `Some(new)` when `desired` is longer than `current`, `None` when it
/// is equal or shorter. `Duration::ZERO` is NATS's "no expiry", i.e. the
/// longest possible retention, so it dominates any finite value in both
/// directions.
// Only `reconcile_bucket_max_age` (cfg `not(coverage)`) and the unit tests call
// this; a coverage build without tests would otherwise see it as dead.
#[cfg_attr(coverage, allow(dead_code))]
fn widened_max_age(current: Duration, desired: Duration) -> Option<Duration> {
    let current_never_expires = current.is_zero();
    let desired_never_expires = desired.is_zero();

    if current_never_expires {
        None
    } else if desired_never_expires {
        Some(Duration::ZERO)
    } else if desired > current {
        Some(desired)
    } else {
        None
    }
}

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
    /// startup, growing it so raising stream retention takes effect. It never
    /// shrinks the bucket: source streams do not shrink their own retention, so
    /// a lowered config must not expire claims that older, still-deliverable
    /// messages reference.
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

/// Grow the retention of an already-provisioned bucket toward `max_age`.
/// A NATS object-store bucket is a stream named `OBJ_<bucket>`; only `max_age`
/// is touched, leaving every other stream setting untouched. The retention only
/// widens (see [`widened_max_age`]), so a lowered config is a no-op.
#[cfg(not(coverage))]
async fn reconcile_bucket_max_age(
    js: &async_nats::jetstream::Context,
    bucket: &str,
    max_age: Duration,
) -> Result<(), ProvisionObjectStoreError> {
    let stream = js
        .get_stream(format!("OBJ_{bucket}"))
        .await
        .map_err(ProvisionObjectStoreError::GetStream)?;
    let mut config = stream.cached_info().config.clone();
    let Some(widened) = widened_max_age(config.max_age, max_age) else {
        return Ok(());
    };
    config.max_age = widened;
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

#[cfg(test)]
mod tests;

#[cfg(all(test, not(coverage), feature = "test-support"))]
mod integration_tests;
