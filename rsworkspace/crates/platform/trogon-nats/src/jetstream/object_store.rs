use std::error::Error;
use std::future::Future;
use std::time::Duration;

use tokio::io::AsyncRead;

use super::claim_bucket::ClaimBucket;

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

/// An object-store handle together with the claim bucket it was opened on.
///
/// A handle cannot be asked which bucket it reads, so the two have to travel as
/// one. Outside tests the only way to get a binding is to open the bucket, which
/// is what stops a consumer from validating one bucket while reading another.
pub struct ClaimBucketBinding<S> {
    store: S,
    bucket: ClaimBucket,
}

impl<S> ClaimBucketBinding<S> {
    pub fn bucket(&self) -> &ClaimBucket {
        &self.bucket
    }

    pub(crate) fn into_parts(self) -> (S, ClaimBucket) {
        (self.store, self.bucket)
    }

    /// Pair a store with a bucket name directly, for tests that redeem claims
    /// from a fake store and so have no bucket to open.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(store: S, bucket: ClaimBucket) -> Self {
        Self { store, bucket }
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

#[derive(Clone)]
pub struct NatsObjectStore {
    store: async_nats::jetstream::object_store::ObjectStore,
}

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

    /// Open an existing claim bucket without creating it or touching its
    /// retention. A consumer redeeming claims reads a bucket whose lifecycle
    /// belongs to the publisher that fills it, so creating one here would only
    /// hide the fact that the publisher never ran.
    ///
    /// The bucket comes back bound to the handle, because the name is what a
    /// consumer checks incoming claims against and a handle it does not match is
    /// worse than no handle at all.
    pub async fn bind_claim_bucket(
        js: &async_nats::jetstream::Context,
        bucket: ClaimBucket,
    ) -> Result<ClaimBucketBinding<Self>, ProvisionObjectStoreError> {
        let store = js
            .get_object_store(bucket.as_str())
            .await
            .map_err(ProvisionObjectStoreError::Get)?;
        Ok(ClaimBucketBinding {
            store: Self { store },
            bucket,
        })
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
    ///
    /// The bucket comes back bound to the handle, so a publisher stamps claims
    /// with the bucket it actually wrote them to.
    pub async fn provision_claim_bucket(
        js: &async_nats::jetstream::Context,
        bucket: ClaimBucket,
        retention: super::claim_retention::ClaimRetention,
    ) -> Result<ClaimBucketBinding<Self>, ProvisionObjectStoreError> {
        let max_age = retention.bucket_max_age();
        let store = Self::provision(
            js,
            async_nats::jetstream::object_store::Config {
                bucket: bucket.as_str().to_string(),
                max_age,
                ..Default::default()
            },
        )
        .await?;
        reconcile_bucket_max_age(js, bucket.as_str(), max_age).await?;
        Ok(ClaimBucketBinding { store, bucket })
    }
}

/// Grow the retention of an already-provisioned bucket toward `max_age`.
/// A NATS object-store bucket is a stream named `OBJ_<bucket>`; only `max_age`
/// is touched, leaving every other stream setting untouched. The retention only
/// widens (see [`widened_max_age`]), so a lowered config is a no-op.
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

impl ObjectStorePut for NatsObjectStore {
    type Error = async_nats::jetstream::object_store::PutError;
    type Info = async_nats::jetstream::object_store::ObjectInfo;

    async fn put<R: AsyncRead + Unpin + Send>(&self, name: &str, data: &mut R) -> Result<Self::Info, Self::Error> {
        self.store.put(name, data).await
    }
}

impl ObjectStoreGet for NatsObjectStore {
    type Error = async_nats::jetstream::object_store::GetError;
    type Reader = async_nats::jetstream::object_store::Object;

    async fn get(&self, name: &str) -> Result<Self::Reader, Self::Error> {
        self.store.get(name).await
    }
}

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "test-support"))]
mod integration_tests;
