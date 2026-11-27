use std::time::Duration;

use bytes::Bytes;
use serde::Serialize;
use serde::de::DeserializeOwned;
use trogon_sandbox_protocol::NatsConnectError;
use trogon_sandbox_protocol::nats_connect;

use crate::{Bus, ClientError, DEFAULT_REQUEST_TIMEOUT, NatsBus, TransportError};

/// A typed client for the sandbox broker's request and reply subjects.
pub struct SandboxClient {
    pub(crate) bus: Box<dyn Bus>,
    pub(crate) api_key: Option<String>,
    pub(crate) default_timeout: Duration,
}

impl SandboxClient {
    /// Connects to the NATS server at `nats_url`.
    pub async fn connect(nats_url: &str) -> Result<Self, NatsConnectError> {
        let client = nats_connect::connect(nats_url).await?;
        Ok(Self::from_client(client))
    }

    pub fn from_client(client: async_nats::Client) -> Self {
        Self::from_bus(Box::new(NatsBus::new(client)))
    }

    /// Builds a client over any [`Bus`].
    pub fn from_bus(bus: Box<dyn Bus>) -> Self {
        Self {
            bus,
            api_key: None,
            default_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }

    /// Authenticates every request with `key` as a bearer token.
    pub fn with_api_key(mut self, key: String) -> Self {
        self.api_key = Some(key);
        self
    }

    /// Sets the reply deadline for calls that take no explicit timeout, such as polling a command or reading a
    /// snapshot tag. Defaults to [`DEFAULT_REQUEST_TIMEOUT`].
    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    pub(crate) fn build_auth_headers(&self) -> Option<async_nats::HeaderMap> {
        let key = self.api_key.as_ref()?;
        let mut headers = async_nats::HeaderMap::new();
        headers.insert("Authorization", format!("Bearer {key}").as_str());
        Some(headers)
    }

    /// Sends raw `payload` on `subject`. No reply before `timeout` is [`ClientError::Timeout`].
    pub(crate) async fn call_bytes(
        &self,
        subject: &str,
        payload: Vec<u8>,
        timeout: Duration,
    ) -> Result<Bytes, ClientError> {
        let reply = self
            .bus
            .request(subject, Bytes::from(payload), timeout, self.build_auth_headers())
            .await?;
        reply.ok_or(ClientError::Timeout(timeout))
    }

    /// Encodes `request`, sends it on `subject`, and decodes the reply.
    pub(crate) async fn send<Req: Serialize, Resp: DeserializeOwned>(
        &self,
        subject: &str,
        request: &Req,
        timeout: Duration,
    ) -> Result<Resp, ClientError> {
        let payload = serde_json::to_vec(request).map_err(TransportError::Encode)?;
        let bytes = self.call_bytes(subject, payload, timeout).await?;
        decode(&bytes)
    }

    /// Like [`Self::send`] with the client's default timeout.
    pub(crate) async fn send_default<Req: Serialize, Resp: DeserializeOwned>(
        &self,
        subject: &str,
        request: &Req,
    ) -> Result<Resp, ClientError> {
        self.send(subject, request, self.default_timeout).await
    }
}

/// Decodes a broker reply.
pub(crate) fn decode<Resp: DeserializeOwned>(bytes: &[u8]) -> Result<Resp, ClientError> {
    serde_json::from_slice(bytes).map_err(|error| TransportError::Decode(error).into())
}
