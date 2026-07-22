use std::future::Future;

use bytes::Bytes;

pub trait CatalogStore: Send + Sync + Clone + 'static {
    type GetError: std::error::Error + Send + Sync + 'static;
    type PutError: std::error::Error + Send + Sync + 'static;

    fn get(&self, key: &str) -> impl Future<Output = Result<Option<Bytes>, Self::GetError>> + Send;
    fn put(&self, key: &str, value: Bytes) -> impl Future<Output = Result<(), Self::PutError>> + Send;
}

impl CatalogStore for async_nats::jetstream::kv::Store {
    type GetError = async_nats::jetstream::kv::EntryError;
    type PutError = async_nats::jetstream::kv::PutError;

    async fn get(&self, key: &str) -> Result<Option<Bytes>, Self::GetError> {
        async_nats::jetstream::kv::Store::get(self, key).await
    }

    async fn put(&self, key: &str, value: Bytes) -> Result<(), Self::PutError> {
        async_nats::jetstream::kv::Store::put(self, key, value).await?;
        Ok(())
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod mock {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    pub struct MockCatalogStore {
        data: Arc<Mutex<HashMap<String, Bytes>>>,
    }

    impl MockCatalogStore {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn snapshot(&self) -> HashMap<String, Bytes> {
            self.data.lock().unwrap().clone()
        }
    }

    impl CatalogStore for MockCatalogStore {
        type GetError = std::convert::Infallible;
        type PutError = std::convert::Infallible;

        async fn get(&self, key: &str) -> Result<Option<Bytes>, Self::GetError> {
            Ok(self.data.lock().unwrap().get(key).cloned())
        }

        async fn put(&self, key: &str, value: Bytes) -> Result<(), Self::PutError> {
            self.data.lock().unwrap().insert(key.to_string(), value);
            Ok(())
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub use mock::MockCatalogStore;
