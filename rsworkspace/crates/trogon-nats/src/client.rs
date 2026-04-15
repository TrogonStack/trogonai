use async_nats::subject::ToSubject;
use async_nats::{Client as NatsAsyncClient, HeaderMap, Message};
use bytes::Bytes;
use futures::Stream;
use std::error::Error;
use std::future::Future;

pub trait SubscribeClient: Send + Sync + Clone + 'static {
    type SubscribeError: Error + Send + Sync;
    type Subscription: Stream<Item = Message> + Unpin + Send + 'static;

    fn subscribe<S: ToSubject + Send>(
        &self,
        subject: S,
    ) -> impl Future<Output = Result<Self::Subscription, Self::SubscribeError>> + Send;
}

pub trait RequestClient: Send + Sync + Clone + 'static {
    type RequestError: Error + Send + Sync;

    fn request_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> impl Future<Output = Result<Message, Self::RequestError>> + Send;
}

pub trait PublishClient: Send + Sync + Clone + 'static {
    type PublishError: Error + Send + Sync;

    fn publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> impl Future<Output = Result<(), Self::PublishError>> + Send;

    /// Returns the server's maximum message payload size in bytes.
    /// Defaults to 1 MiB (the NATS server default).
    fn max_payload(&self) -> usize {
        1024 * 1024
    }
}

pub trait FlushClient: Send + Sync + Clone + 'static {
    type FlushError: Error + Send + Sync;

    fn flush(&self) -> impl Future<Output = Result<(), Self::FlushError>> + Send;
}

impl SubscribeClient for NatsAsyncClient {
    type SubscribeError = async_nats::client::SubscribeError;
    type Subscription = async_nats::Subscriber;

    async fn subscribe<S: ToSubject + Send>(
        &self,
        subject: S,
    ) -> Result<Self::Subscription, Self::SubscribeError> {
        self.subscribe(subject).await
    }
}

impl RequestClient for NatsAsyncClient {
    type RequestError = async_nats::client::RequestError;

    async fn request_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> Result<Message, Self::RequestError> {
        self.request_with_headers(subject, headers, payload).await
    }
}

impl PublishClient for NatsAsyncClient {
    type PublishError = async_nats::client::PublishError;

    async fn publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> Result<(), Self::PublishError> {
        self.publish_with_headers(subject, headers, payload).await
    }

    fn max_payload(&self) -> usize {
        self.server_info().max_payload
    }
}

impl FlushClient for NatsAsyncClient {
    type FlushError = async_nats::client::FlushError;

    async fn flush(&self) -> Result<(), Self::FlushError> {
        self.flush().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    /// A minimal stub that relies entirely on the default `max_payload()` impl.
    #[derive(Clone)]
    struct StubPublisher;

    impl PublishClient for StubPublisher {
        type PublishError = std::io::Error;

        async fn publish_with_headers<S: ToSubject + Send>(
            &self,
            _subject: S,
            _headers: HeaderMap,
            _payload: Bytes,
        ) -> Result<(), Self::PublishError> {
            Ok(())
        }
        // max_payload() NOT overridden — uses the 1 MiB default
    }

    #[test]
    fn publish_client_default_max_payload_is_one_mib() {
        let stub = StubPublisher;
        assert_eq!(stub.max_payload(), 1024 * 1024);
    }
}
