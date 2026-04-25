use std::fmt;
use std::future::Future;

use async_nats::jetstream::AckKind;

pub trait JsMessageRef: Send + 'static {
    fn message(&self) -> &async_nats::Message;
}

pub trait JsAck: Send + 'static {
    type Error: fmt::Display + fmt::Debug + Send + Sync;

    fn ack(&self) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

pub trait JsAckWith: Send + 'static {
    type Error: fmt::Display + fmt::Debug + Send + Sync;

    fn ack_with(&self, kind: AckKind) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

pub trait JsDoubleAck: Send + 'static {
    type Error: fmt::Display + fmt::Debug + Send + Sync;

    fn double_ack(&self) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

pub trait JsDoubleAckWith: Send + 'static {
    type Error: fmt::Display + fmt::Debug + Send + Sync;

    fn double_ack_with(&self, kind: AckKind) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

pub trait JsRequestMessage: JsMessageRef + JsAck + JsAckWith {}
impl<T: JsMessageRef + JsAck + JsAckWith> JsRequestMessage for T {}

pub trait JsDispatchMessage: JsMessageRef + JsAck + JsAckWith {}
impl<T: JsMessageRef + JsAck + JsAckWith> JsDispatchMessage for T {}
