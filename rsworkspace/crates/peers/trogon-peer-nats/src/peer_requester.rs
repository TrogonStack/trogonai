use crate::{PeerMethod, PeerRequestError, PeerRequesterConfig};

#[derive(Clone)]
pub struct PeerRequester {
    _client: async_nats::Client,
    config: PeerRequesterConfig,
}

impl PeerRequester {
    pub fn new(client: async_nats::Client, config: PeerRequesterConfig) -> Self {
        Self {
            _client: client,
            config,
        }
    }

    pub fn config(&self) -> &PeerRequesterConfig {
        &self.config
    }

    pub async fn request(
        &self,
        _method: PeerMethod,
        _payload: Vec<u8>,
    ) -> Result<async_nats::Message, PeerRequestError> {
        Err(PeerRequestError::NotImplemented)
    }
}

#[cfg(test)]
mod tests;
