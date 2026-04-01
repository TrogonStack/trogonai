#[cfg(not(coverage))]
pub mod client;
pub mod message;
pub mod traits;

#[cfg(feature = "test-support")]
pub mod mocks;

#[cfg(not(coverage))]
pub use client::{
    ConsumerError, GetStreamError, MessagesError, NatsJetStreamClient, NatsJetStreamConsumer,
    PublishAckFuture, PublishError, StreamError,
};
pub use message::{
    JsAck, JsAckWith, JsDispatchMessage, JsDoubleAck, JsDoubleAckWith, JsMessageRef,
    JsRequestMessage,
};
pub use traits::{
    JetStreamConsumer, JetStreamContext, JetStreamCreateConsumer, JetStreamGetStream,
    JetStreamPublisher,
};

#[cfg(feature = "test-support")]
pub use mocks::{
    AckKindSnapshot, AckKindValue, MockJetStreamConsumer, MockJetStreamConsumerFactory,
    MockJetStreamContext, MockJetStreamPublisher, MockJetStreamStream, MockJsMessage,
};
