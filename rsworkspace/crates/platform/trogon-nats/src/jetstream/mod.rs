pub mod claim_bucket;
pub mod claim_check;
pub mod claim_retention;
pub mod client;
pub mod conditional_kv;
pub mod create_conflicts;
pub mod message;
mod native_kv_cas_error;
mod native_kv_cas_stage;
mod native_kv_expectation;
mod native_kv_key;
mod native_kv_scope;
mod native_kv_update;
mod native_stream_get_request;
mod native_stream_get_response;
pub mod not_found;
pub mod object_store;
pub mod processor;
pub mod publish;
pub mod stream_max_age;
pub mod strict_kv;
pub mod traits;

#[cfg(any(test, feature = "test-support"))]
pub mod mocks;

pub use crate::constants::{DEFAULT_CLAIM_BUCKET, HEADER_CLAIM_BUCKET, HEADER_CLAIM_CHECK, HEADER_CLAIM_KEY};
pub use claim_bucket::{ClaimBucket, ClaimBucketError, ClaimBucketHeader};
pub use claim_check::{ClaimCheckPublisher, ClaimResolveError, ClaimResolver, MaxPayload, is_claim, resolve_claim};
pub use claim_retention::ClaimRetention;
pub use client::{
    ConsumerError, GetStreamError, MessagesError, NatsJetStreamClient, NatsJetStreamConsumer, PublishAckFuture,
    PublishError, StreamError,
};
pub use conditional_kv::conditional_native_kv_update;
pub use create_conflicts::{
    is_create_key_already_exists, is_create_key_value_already_exists, is_create_stream_already_exists,
};
pub use message::{JsAck, JsAckWith, JsDispatchMessage, JsDoubleAck, JsDoubleAckWith, JsMessageRef, JsRequestMessage};
pub use native_kv_cas_error::NativeKvCasError;
pub use native_kv_cas_stage::NativeKvCasStage;
pub use native_kv_expectation::NativeKvExpectation;
pub use native_kv_key::NativeKvKey;
pub use native_kv_scope::NativeKvScope;
pub use native_kv_update::NativeKvUpdate;
pub use not_found::{is_get_key_value_not_found, is_get_stream_not_found};
pub use object_store::NatsObjectStore;
pub use object_store::{ClaimBucketBinding, ObjectStoreGet, ObjectStorePut};
pub use processor::{HandlerVerdict, MessageHandler, PoisonReason, Processor, ProcessorError, RedeliveryPolicy};
pub use publish::{PublishOutcome, publish_event};
pub use stream_max_age::StreamMaxAge;
pub use strict_kv::{
    JetStreamStrictKvRead, NativeKvReadError, StrictNativeKvStore, strict_native_kv_operation,
    strict_native_kv_raw_operation,
};
pub use traits::{
    JetStreamConsumer, JetStreamContext, JetStreamCreateConsumer, JetStreamCreateKeyValue, JetStreamGetKeyValue,
    JetStreamGetRawMessage, JetStreamGetStream, JetStreamGetStreamInfo, JetStreamKeyValueCreateWithTtl,
    JetStreamKeyValueDeleteExpectRevision, JetStreamKeyValueStatus, JetStreamKeyValueUpdate, JetStreamKvCreate,
    JetStreamKvEntry, JetStreamKvGet, JetStreamKvKeys, JetStreamLastRawMessageBySubject, JetStreamPublishMessage,
    JetStreamPublisher, JetStreamSubjectPurger, JsMessageOf, ProvisionedStreamField, PurgeOutcome,
    reconciled_stream_config,
};

#[cfg(any(test, feature = "test-support"))]
pub use mocks::{
    AckKindSnapshot, AckKindValue, MockJetStreamConsumer, MockJetStreamConsumerFactory, MockJetStreamContext,
    MockJetStreamKvClient, MockJetStreamKvStore, MockJetStreamPublishMessage, MockJetStreamPublisher,
    MockJetStreamPurger, MockJetStreamStream, MockJsMessage, MockKvEntryOutcome, MockKvGetOutcome, MockObjectStore,
    MockPublishedOutboundMessage, MockStreamProvisionError,
};

mod strict_stream;
pub use strict_stream::{NativeStreamReadError, StrictNativeStreamReader, parse_native_stored_headers};
