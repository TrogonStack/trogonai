#[cfg(not(coverage))]
pub mod client;
pub mod message;
pub mod traits;

#[cfg(feature = "test-support")]
pub mod mocks;

#[cfg(not(coverage))]
pub use client::{JetStreamError, NatsJetStreamClient, NatsJetStreamConsumer, NatsJsMessage};
pub use message::{JsMessage, JsSignal};
pub use traits::{
    JetStreamConsumer, JetStreamConsumerFactory, JetStreamContext, JetStreamPublisher, NoJetStream,
    NoOpConsumer, NoOpMessage,
};

#[cfg(feature = "test-support")]
pub use mocks::{
    MockJetStreamConsumer, MockJetStreamConsumerFactory, MockJetStreamContext,
    MockJetStreamPublisher, MockJsMessage,
};
