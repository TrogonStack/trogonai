use std::time::Duration;

use trogon_sandbox_contract::{ProviderLease, SandboxId};

use crate::CheckFailureError;

/// Why a worker's lease handler is not idempotent under a re-sent lease id.
#[derive(Debug, thiserror::Error)]
pub enum RedispatchError {
    /// The lease request could not be built.
    #[error("could not build the lease request: {0}")]
    Request(#[from] CheckFailureError),
    /// The dispatch could not be encoded.
    #[error("could not encode the dispatch: {0}")]
    Encode(#[source] serde_json::Error),
    /// The reply inbox could not be subscribed.
    #[error("could not subscribe to the reply inbox: {0}")]
    Subscribe(#[source] async_nats::SubscribeError),
    /// The handler under test failed.
    #[error("the handler failed: {0}")]
    Handler(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// The handler sent no reply in time.
    #[error("the handler sent no reply within {0:?}")]
    NoReply(Duration),
    /// The reply is not a grant.
    #[error("the reply is not a granted lease: {0}")]
    Decode(#[source] serde_json::Error),
    /// The repeat dispatch was answered with another grant than the first.
    #[error("the repeat dispatch was granted {second:?}, not the first grant {first:?}")]
    RepeatGrantDiffers {
        first: Box<ProviderLease>,
        second: Box<ProviderLease>,
    },
    /// The provider launched another number of sandboxes than the dispatches call for.
    #[error("expected {expected} launches, observed {observed}")]
    LaunchCount { expected: u32, observed: u32 },
    /// A lease id reused after a cancel was answered with the released sandbox.
    #[error("a released lease id was answered with the released sandbox {0:?}")]
    ReleasedSandboxReused(SandboxId),
}
