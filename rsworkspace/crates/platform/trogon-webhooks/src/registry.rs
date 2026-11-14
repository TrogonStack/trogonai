use trogon_nats::jetstream::JetStreamKvEntry;

use crate::{EndpointId, EndpointLookup, RegisteredEndpoint, RegistryError};

#[derive(Clone)]
pub struct NatsEndpointRegistry<K> {
    store: K,
}

impl<K> NatsEndpointRegistry<K> {
    pub fn new(store: K) -> Self {
        Self { store }
    }
    pub fn store(&self) -> &K {
        &self.store
    }
}

impl<K: JetStreamKvEntry> EndpointLookup for NatsEndpointRegistry<K> {
    async fn lookup(&self, _endpoint_id: &EndpointId) -> Result<Option<RegisteredEndpoint>, RegistryError> {
        Ok(None)
    }
}
