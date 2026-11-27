use crate::{BrokerSetupError, ServiceError};

/// The broker could not start serving.
#[derive(Debug, thiserror::Error)]
pub enum BrokerRunError {
    #[error("could not connect to NATS")]
    Connect(#[from] async_nats::ConnectError),
    #[error(transparent)]
    Setup(#[from] BrokerSetupError),
    #[error(transparent)]
    Service(#[from] ServiceError),
}
