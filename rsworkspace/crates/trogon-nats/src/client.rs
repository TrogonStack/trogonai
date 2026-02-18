use async_nats::subject::ToSubject;
use async_nats::{Client as NatsAsyncClient, HeaderMap, Message, Subscriber};
use bytes::Bytes;
use std::error::Error;
use std::future::Future;

pub trait SubscribeClient: Send + Sync + Clone + 'static {
    type SubscribeError: Error + Send + Sync;

    fn subscribe<S: ToSubject + Send>(
        &self,
        subject: S,
    ) -> impl Future<Output = Result<Subscriber, Self::SubscribeError>> + Send;
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
}

pub trait FlushClient: Send + Sync + Clone + 'static {
    type FlushError: Error + Send + Sync;

    fn flush(&self) -> impl Future<Output = Result<(), Self::FlushError>> + Send;
}

impl SubscribeClient for NatsAsyncClient {
    type SubscribeError = async_nats::client::SubscribeError;

    async fn subscribe<S: ToSubject + Send>(
        &self,
        subject: S,
    ) -> Result<Subscriber, Self::SubscribeError> {
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
}

impl FlushClient for NatsAsyncClient {
    type FlushError = async_nats::client::FlushError;

    async fn flush(&self) -> Result<(), Self::FlushError> {
        self.flush().await
    }
}
