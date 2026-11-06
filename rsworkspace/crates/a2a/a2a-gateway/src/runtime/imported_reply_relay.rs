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
enum RelayError {
    #[error(transparent)]
    Request(#[from] async_nats::RequestError),
    #[error(transparent)]
    Publish(#[from] async_nats::client::PublishError),
    #[error(transparent)]
    Deadline(#[from] tokio::time::error::Elapsed),
}

impl ImportedReplyRelay {
    fn new(capacity: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(capacity)),
        }
    }

    pub(super) fn forward(
        &self,
        client: Client,
        subject: Subject,
        reply: Subject,
        request: Request,
        deadline: Duration,
        shutdown: CancellationToken,
    ) -> Result<(), TryAcquireError> {
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
                _ = shutdown.cancelled() => {}
                result = tokio::time::timeout(deadline, relay) => {
                    if let Err(error) = result.map_err(RelayError::Deadline).and_then(std::convert::identity) {
                        tracing::warn!(error = %error, "gateway imported reply relay failed");
                    }
                }
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests;
