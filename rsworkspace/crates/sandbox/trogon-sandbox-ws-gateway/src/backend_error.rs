use trogon_sandbox_client::ClientError;

use crate::ObservabilityError;

/// A backend operation failed outside the broker's typed answers.
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("subscribing to the event stream failed")]
    Subscribe(Box<ClientError>),
    #[error("reading the observability state failed")]
    Observability(#[from] ObservabilityError),
}

impl From<ClientError> for BackendError {
    fn from(error: ClientError) -> Self {
        Self::Subscribe(Box::new(error))
    }
}
