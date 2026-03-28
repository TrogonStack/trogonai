use std::time::Duration;

use async_nats::jetstream;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsSignal {
    Ack,
    DoubleAck,
    Nak,
    NakWithDelay(Duration),
    Progress,
    Term,
}

pub struct JsMessage {
    #[cfg_attr(coverage, allow(dead_code))]
    inner: jetstream::Message,
}

#[cfg(not(coverage))]
impl JsMessage {
    pub fn new(inner: jetstream::Message) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> jetstream::Message {
        self.inner
    }

    pub fn message(&self) -> &async_nats::Message {
        &self.inner.message
    }

    pub fn payload(&self) -> &bytes::Bytes {
        &self.inner.payload
    }

    pub fn subject(&self) -> &str {
        self.inner.subject.as_str()
    }

    pub fn headers(&self) -> Option<&async_nats::HeaderMap> {
        self.inner.headers.as_ref()
    }

    pub fn reply(&self) -> Option<&async_nats::Subject> {
        self.inner.reply.as_ref()
    }

    pub async fn ack(&self) -> Result<(), async_nats::Error> {
        self.inner.ack().await
    }

    pub async fn double_ack(&self) -> Result<(), async_nats::Error> {
        self.inner.double_ack().await
    }

    pub async fn nak(&self) -> Result<(), async_nats::Error> {
        self.inner.ack_with(jetstream::AckKind::Nak(None)).await
    }

    pub async fn nak_with_delay(&self, delay: Duration) -> Result<(), async_nats::Error> {
        self.inner
            .ack_with(jetstream::AckKind::Nak(Some(delay)))
            .await
    }

    pub async fn term(&self) -> Result<(), async_nats::Error> {
        self.inner.ack_with(jetstream::AckKind::Term).await
    }

    pub async fn in_progress(&self) -> Result<(), async_nats::Error> {
        self.inner.ack_with(jetstream::AckKind::Progress).await
    }

    pub fn info(&self) -> Result<jetstream::message::Info<'_>, async_nats::Error> {
        self.inner.info()
    }
}
