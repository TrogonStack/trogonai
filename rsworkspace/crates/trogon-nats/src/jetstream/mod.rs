#[cfg(not(coverage))]
pub mod client;
pub mod message;
pub mod publish;
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
pub use publish::{PublishOutcome, publish_event};
pub use traits::{
    JetStreamConsumer, JetStreamContext, JetStreamCreateConsumer, JetStreamGetStream,
    JetStreamPublisher, JsMessageOf,
};

#[cfg(feature = "test-support")]
pub use mocks::{
    AckKindSnapshot, AckKindValue, MockJetStreamConsumer, MockJetStreamConsumerFactory,
    MockJetStreamContext, MockJetStreamPublisher, MockJetStreamStream, MockJsMessage,
};
