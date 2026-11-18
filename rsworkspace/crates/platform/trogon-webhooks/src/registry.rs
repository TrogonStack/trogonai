use async_nats::jetstream::kv::Operation;
use buffa::Message as _;
use trogon_nats::jetstream::JetStreamKvEntry;
use trogonai_proto::gateway::webhooks::v1;

use crate::{
    ConfigurationRevision, EndpointId, EndpointLookup, EndpointRegistration, RegisteredEndpoint, RegistryError,
};

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
    async fn lookup(&self, endpoint_id: &EndpointId) -> Result<Option<RegisteredEndpoint>, RegistryError> {
        let Some(entry) = self
            .store
            .entry(endpoint_id.as_str().to_owned())
            .await
            .map_err(RegistryError::Read)?
        else {
            return Ok(None);
        };
        if entry.operation != Operation::Put {
            return Ok(None);
        }
        if entry.value.len() > 4096 {
            return Err(RegistryError::Oversized);
        }
        let wire = v1::EndpointRegistration::decode_from_slice(&entry.value).map_err(RegistryError::Decode)?;
        let registration = EndpointRegistration::try_from(wire).map_err(RegistryError::Registration)?;
        if registration.endpoint_id() != endpoint_id {
            return Err(RegistryError::IdentityMismatch);
        }
        let revision = ConfigurationRevision::new(entry.revision).map_err(RegistryError::Revision)?;
        Ok(Some(RegisteredEndpoint::new(registration, revision)))
    }
}
