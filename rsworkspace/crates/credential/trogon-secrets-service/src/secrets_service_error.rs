use async_nats::client::SubscribeError;
use async_nats::jetstream::consumer::StreamError;
use async_nats::jetstream::context::GetStreamError;
use async_nats::jetstream::stream::ConsumerError;
use tokio::task::JoinError;
use trogon_nats::{ConnectError, NatsToken};
use trogon_secret_store::OpenBaoLoginError;

use super::constants::INVALIDATION_STREAM;
use super::{ConfigSetting, InvalidationPublisherError, InvalidationStreamMismatchError, ReleaseServerError};

/// Why the secrets service refused to start, or stopped serving before it was
/// told to. Either way it exits rather than serve without one of the parts a
/// release needs.
#[derive(Debug, thiserror::Error)]
pub enum SecretsServiceError {
    #[error("the NATS connection {} names cannot be used", ConfigSetting::NatsUrl)]
    Connect(#[source] ConnectError),
    #[error(
        "credential stream {stream}, named by {}, cannot be read",
        ConfigSetting::CredentialStream
    )]
    CredentialStream {
        stream: NatsToken,
        #[source]
        source: GetStreamError,
    },
    /// The service checks the invalidation stream and never creates it, so a
    /// deployment that has not provisioned it cannot announce a revocation.
    #[error(
        "invalidation stream '{INVALIDATION_STREAM}' cannot be read; the secrets service checks it and never creates it"
    )]
    InvalidationStream(#[source] GetStreamError),
    #[error(
        "invalidation stream '{INVALIDATION_STREAM}' would weaken a guarantee holders rely on, checked with the replicas {} asks for",
        ConfigSetting::InvalidationStreamReplicas
    )]
    InvalidationStreamMismatch(#[source] InvalidationStreamMismatchError),
    #[error(
        "the OpenBao {} names cannot be used for the AppRole login",
        ConfigSetting::OpenBaoAddress
    )]
    OpenBaoLogin(#[source] OpenBaoLoginError),
    #[error("the release endpoint stopped serving")]
    Serve(#[source] ReleaseServerError<SubscribeError>),
    /// A service that can no longer announce revocations stops releasing
    /// values whose revocation would go unannounced (ADR#0074).
    #[error("the invalidation publisher stopped publishing")]
    Publish(#[source] InvalidationPublisherError<ConsumerError, StreamError>),
    #[error("the audit writer stopped before its final drain")]
    AuditDrain(#[source] JoinError),
}
