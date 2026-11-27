use std::time::Duration;

use async_nats::Client;
use async_trait::async_trait;
use bytes::Bytes;

use crate::bus::BrokerBus;
use crate::constants::FILE_TRANSFER_TIMEOUT;

pub struct NatsBus {
    client: Client,
}

impl NatsBus {
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

fn is_file_transfer(subject: &str) -> bool {
    subject.contains(".write_files.") || subject.contains(".read_file.") || subject.contains(".file_op.")
}

#[async_trait]
impl BrokerBus for NatsBus {
    async fn request(&self, subject: &str, payload: Bytes) -> anyhow::Result<Bytes> {
        if is_file_transfer(subject) {
            let reply = trogon_sandbox_protocol::nats_transfer::request(
                &self.client,
                subject,
                payload,
                None,
                FILE_TRANSFER_TIMEOUT,
            )
            .await?;
            return Ok(reply);
        }
        let subject = subject.to_owned();
        let msg = self.client.request(subject, payload).await?;
        Ok(msg.payload)
    }

    async fn request_with_timeout(&self, subject: &str, payload: Bytes, timeout: Duration) -> anyhow::Result<Bytes> {
        let request = async_nats::Request::new().payload(payload).timeout(Some(timeout));
        let msg = self.client.send_request(subject.to_owned(), request).await?;
        Ok(msg.payload)
    }

    async fn publish(&self, subject: &str, payload: Bytes) -> anyhow::Result<()> {
        let subject = subject.to_owned();
        self.client.publish(subject, payload).await?;
        Ok(())
    }

    async fn publish_with_msg_id(&self, subject: &str, msg_id: &str, payload: Bytes) -> anyhow::Result<()> {
        let mut headers = async_nats::HeaderMap::new();
        headers.insert(async_nats::header::NATS_MESSAGE_ID, msg_id);
        self.client
            .publish_with_headers(subject.to_owned(), headers, payload)
            .await?;
        Ok(())
    }

    async fn request_with_auth(
        &self,
        subject: &str,
        auth: &str,
        payload: Bytes,
        timeout: Option<Duration>,
    ) -> anyhow::Result<Bytes> {
        let mut headers = async_nats::HeaderMap::new();
        headers.insert(trogon_sandbox_protocol::AUTH_HEADER, auth);
        if timeout.is_none() && is_file_transfer(subject) {
            let reply = trogon_sandbox_protocol::nats_transfer::request(
                &self.client,
                subject,
                payload,
                Some(headers),
                FILE_TRANSFER_TIMEOUT,
            )
            .await?;
            return Ok(reply);
        }
        let mut request = async_nats::Request::new().payload(payload).headers(headers);
        if timeout.is_some() {
            request = request.timeout(timeout);
        }
        let msg = self.client.send_request(subject.to_owned(), request).await?;
        Ok(msg.payload)
    }

    async fn publish_with_auth(&self, subject: &str, auth: &str, payload: Bytes) -> anyhow::Result<()> {
        let mut headers = async_nats::HeaderMap::new();
        headers.insert(trogon_sandbox_protocol::AUTH_HEADER, auth);
        self.client
            .publish_with_headers(subject.to_owned(), headers, payload)
            .await?;
        Ok(())
    }

    fn max_payload(&self) -> Option<usize> {
        Some(self.client.server_info().max_payload)
    }
}

#[cfg(test)]
mod tests;
