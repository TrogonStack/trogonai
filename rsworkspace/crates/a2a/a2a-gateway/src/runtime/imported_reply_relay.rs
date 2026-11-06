use std::sync::{Arc, LazyLock};
use std::time::Duration;

use async_nats::{Client, Request, Subject};
use tokio::sync::{Semaphore, TryAcquireError};
use tokio_util::sync::CancellationToken;

pub(super) static IMPORTED_REPLY_RELAY: LazyLock<ImportedReplyRelay> = LazyLock::new(|| ImportedReplyRelay::new(64));

pub(super) struct ImportedReplyRelay {
    permits: Arc<Semaphore>,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum RelayError {
    #[error(transparent)]
    Request(#[from] async_nats::RequestError),
    #[error(transparent)]
    Publish(#[from] async_nats::client::PublishError),
    #[error(transparent)]
    Deadline(#[from] tokio::time::error::Elapsed),
}

impl RelayError {
    pub(super) fn code(&self) -> i32 {
        match self {
            Self::Deadline(_) => -32_800,
            Self::Request(_) | Self::Publish(_) => -32_803,
        }
    }

    pub(super) fn message(&self) -> &'static str {
        match self {
            Self::Deadline(_) => "gateway imported reply deadline exceeded",
            Self::Request(error) if error.kind() == async_nats::RequestErrorKind::NoResponders => {
                "gateway agent has no responders"
            }
            Self::Request(_) => "gateway agent request failed",
            Self::Publish(_) => "gateway caller reply publish failed",
        }
    }
}

impl ImportedReplyRelay {
    fn new(capacity: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(capacity)),
        }
    }

    /// Reserves capacity before forwarding and reports the completed relay outcome.
    /// Cancellation drops the request without inventing an agent error or retrying it.
    /// The callback owns delivery auditing; admission is not a successful delivery.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn forward<F, Fut>(
        &self,
        client: Client,
        subject: Subject,
        reply: Subject,
        request: Request,
        deadline: Duration,
        shutdown: CancellationToken,
        complete: F,
    ) -> Result<(), TryAcquireError>
    where
        F: FnOnce(Result<(), RelayError>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let permit = self.permits.clone().try_acquire_owned()?;
        tokio::spawn(async move {
            let _permit = permit;
            // A service-import reply is reserved by NATS. A local request inbox
            // preserves that mapping without blocking unrelated ingress.
            let relay = async {
                let response = client.send_request(subject, request.timeout(None)).await?;
                client
                    .publish_with_headers(reply, response.headers.unwrap_or_default(), response.payload)
                    .await?;
                Ok::<(), RelayError>(())
            };
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => {}
                result = tokio::time::timeout(deadline, relay) => {
                    let outcome = result.map_err(RelayError::Deadline).and_then(std::convert::identity);
                    tokio::select! {
                        biased;
                        _ = shutdown.cancelled() => {}
                        _ = tokio::time::timeout(Duration::from_secs(5), complete(outcome)) => {}
                    }
                }
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests;
