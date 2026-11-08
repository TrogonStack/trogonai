use crate::{
    PeerIngress, PeerMethod, PeerPrincipal, PeerReplySubject, PeerRequestError, PeerRequesterConfig, PeerRoute,
};

#[derive(Clone)]
pub struct PeerRequester {
    client: async_nats::Client,
    config: PeerRequesterConfig,
}

impl PeerRequester {
    pub fn new(client: async_nats::Client, config: PeerRequesterConfig) -> Self {
        Self { client, config }
    }

    pub fn config(&self) -> &PeerRequesterConfig {
        &self.config
    }

    pub async fn request(&self, method: PeerMethod, payload: Vec<u8>) -> Result<async_nats::Message, PeerRequestError> {
        self.request_with_headers(method, async_nats::HeaderMap::new(), payload)
            .await
    }

    pub async fn request_with_headers(
        &self,
        method: PeerMethod,
        headers: async_nats::HeaderMap,
        payload: Vec<u8>,
    ) -> Result<async_nats::Message, PeerRequestError> {
        let principal = PeerPrincipal::new(self.config.account().tenant().clone(), self.config.caller().clone());
        let route = PeerRoute::new(principal.clone(), method);
        let subject = PeerIngress::new(self.config.account().clone()).subject(&route)?;
        let generated_inbox = self.client.new_inbox();
        let nonce = generated_inbox.rsplit('.').next().unwrap_or_default();
        let reply = PeerReplySubject::parse_for(
            &principal,
            &format!(
                "_INBOX.{}.{}.{}.{nonce}",
                principal.tenant().as_str(),
                principal.caller().as_str(),
                self.config.connection().as_str(),
            ),
        )?;
        let timeout = self.config.timeout().as_duration();
        let request = async_nats::Request::new()
            .headers(headers)
            .inbox(reply.as_str().to_owned())
            // The outer deadline includes subscription and publish setup.
            .timeout(None)
            .payload(payload.into());
        let response = tokio::time::timeout(timeout, self.client.send_request(subject, request))
            .await
            .map_err(|source| {
                async_nats::RequestError::with_source(async_nats::RequestErrorKind::TimedOut, source)
            })??;
        Ok(response)
    }
}

#[cfg(test)]
mod tests;
