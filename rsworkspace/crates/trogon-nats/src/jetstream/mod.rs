pub mod claim_check;
#[cfg(not(coverage))]
pub mod client;
pub mod create_conflicts;
pub mod message;
pub mod object_store;
pub mod publish;
pub mod stream_max_age;
pub mod traits;

#[cfg(any(test, feature = "test-support"))]
pub mod mocks;

pub use claim_check::{
    ClaimCheckPublisher, ClaimResolveError, HEADER_CLAIM_BUCKET, HEADER_CLAIM_CHECK, HEADER_CLAIM_KEY, MaxPayload,
    is_claim, resolve_claim,
};
#[cfg(not(coverage))]
pub use client::{
    ConsumerError, GetStreamError, MessagesError, NatsJetStreamClient, NatsJetStreamConsumer, PublishAckFuture,
    PublishError, StreamError,
};
pub use create_conflicts::{is_create_key_value_already_exists, is_create_stream_already_exists};
pub use message::{JsAck, JsAckWith, JsDispatchMessage, JsDoubleAck, JsDoubleAckWith, JsMessageRef, JsRequestMessage};
#[cfg(not(coverage))]
pub use object_store::NatsObjectStore;
pub use object_store::{ObjectStoreGet, ObjectStorePut};
pub use publish::{PublishOutcome, publish_event};
pub use stream_max_age::StreamMaxAge;
pub use traits::{
    JetStreamConsumer, JetStreamContext, JetStreamCreateConsumer, JetStreamCreateKeyValue, JetStreamGetKeyValue,
    JetStreamGetStream, JetStreamKeyValueCreateWithTtl, JetStreamKeyValueDeleteExpectRevision, JetStreamKeyValueStatus,
    JetStreamKeyValueUpdate, JetStreamPublishMessage, JetStreamPublisher, JsMessageOf,
};

#[cfg(any(test, feature = "test-support"))]
pub use mocks::{
    AckKindSnapshot, AckKindValue, MockJetStreamConsumer, MockJetStreamConsumerFactory, MockJetStreamContext,
    MockJetStreamKvClient, MockJetStreamKvStore, MockJetStreamPublisher, MockJetStreamStream, MockJsMessage,
    MockObjectStore,
};
