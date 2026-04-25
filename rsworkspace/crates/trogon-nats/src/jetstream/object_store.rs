use std::error::Error;
use std::future::Future;

use tokio::io::AsyncRead;

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
#[derive(Debug)]
pub enum ProvisionObjectStoreError {
    Create(async_nats::jetstream::context::CreateObjectStoreError),
    Get(async_nats::jetstream::context::ObjectStoreError),
}

#[cfg(not(coverage))]
impl std::fmt::Display for ProvisionObjectStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Create(e) => write!(f, "failed to create object store: {e}"),
            Self::Get(e) => write!(f, "failed to get existing object store: {e}"),
        }
    }
}

#[cfg(not(coverage))]
impl Error for ProvisionObjectStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Create(e) => Some(e),
            Self::Get(e) => Some(e),
        }
    }
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
        use async_nats::jetstream::context::CreateKeyValueErrorKind;

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
