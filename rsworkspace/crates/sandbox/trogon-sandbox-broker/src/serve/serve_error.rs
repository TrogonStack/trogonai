use std::error::Error;

use trogon_sandbox_protocol::NatsConnectError;

use crate::auth_setup::AuthSetupError;
use crate::committed_policies::CommittedPoliciesError;
use crate::metrics::BrokerMetricsError;
use crate::signing_bus::SigningBusError;

/// Why the broker could not start serving.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error("create the broker metrics")]
    Metrics(#[from] BrokerMetricsError),
    #[error("connect to NATS")]
    Connect(#[from] NatsConnectError),
    #[error("open the {store}")]
    Open {
        store: &'static str,
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },
    #[error("set up broker auth")]
    AuthSetup(#[from] AuthSetupError),
    #[error("wrap the broker bus")]
    SigningBus(#[from] SigningBusError),
    #[error("open the committed policy writer")]
    CommittedPolicies(#[from] CommittedPoliciesError),
    #[error("configure the dispatch signer")]
    DispatchSigner {
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },
    #[error("snapshot accounting unavailable")]
    SnapshotAccountingUnavailable,
    #[error("subscribe to {subject}")]
    Subscribe {
        subject: &'static str,
        #[source]
        source: async_nats::SubscribeError,
    },
}

impl ServeError {
    /// Maps a failure to open `store` into [`ServeError::Open`].
    pub(super) fn opening<E>(store: &'static str) -> impl FnOnce(E) -> Self
    where
        E: Into<Box<dyn Error + Send + Sync>>,
    {
        move |source| Self::Open {
            store,
            source: source.into(),
        }
    }
}

#[cfg(test)]
mod tests;
