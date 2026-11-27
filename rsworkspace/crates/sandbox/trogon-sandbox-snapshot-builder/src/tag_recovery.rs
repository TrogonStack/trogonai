use std::sync::Arc;

use async_nats::Client;
use async_nats::jetstream::{self, AckKind, consumer};
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use trogon_sandbox_broker::snapshot_accounting::ensure_publication_stream;
use trogon_sandbox_broker::snapshot_tag::{SnapshotTagStore, SnapshotTagStoreError};
use trogon_sandbox_contract::{SnapshotAccountingDisposition, SnapshotPublished};
use trogon_sandbox_protocol::subjects::SNAPSHOT_PUBLISHED;

use crate::TagRecoveryError;
use crate::accounting_confirmation::confirm_accounting;
use crate::auto_tag::auto_tag_published;
use crate::constants::{
    TAG_RECOVERY_ACK_WAIT, TAG_RECOVERY_CONSUMER, TAG_RECOVERY_MAX_ACK_PENDING, TAG_RECOVERY_RETRY_DELAY,
    TAG_RETRY_BACKOFF,
};
use crate::recovery_attempt_error::RecoveryAttemptError;

/// Moves the tag of every published snapshot whose tag move did not finish, from a durable consumer on the
/// publication stream.
pub struct TagRecovery {
    consumer: consumer::PullConsumer,
    nats: Client,
}

impl TagRecovery {
    pub async fn open(js: jetstream::Context, nats: Client) -> Result<Self, TagRecoveryError> {
        let stream = ensure_publication_stream(&js).await?;
        let consumer = stream
            .get_or_create_consumer(
                TAG_RECOVERY_CONSUMER,
                consumer::pull::Config {
                    durable_name: Some(TAG_RECOVERY_CONSUMER.to_string()),
                    filter_subject: SNAPSHOT_PUBLISHED.to_string(),
                    ack_policy: consumer::AckPolicy::Explicit,
                    ack_wait: TAG_RECOVERY_ACK_WAIT,
                    max_ack_pending: TAG_RECOVERY_MAX_ACK_PENDING,
                    max_deliver: -1,
                    ..Default::default()
                },
            )
            .await?;
        let config = &consumer.cached_info().config;
        let compatible = config.ack_policy == consumer::AckPolicy::Explicit
            && config.durable_name.as_deref() == Some(TAG_RECOVERY_CONSUMER)
            && config.deliver_policy == consumer::DeliverPolicy::All
            && !config.headers_only
            && config.max_deliver <= 0
            && (1..=TAG_RECOVERY_MAX_ACK_PENDING).contains(&config.max_ack_pending)
            && config.inactive_threshold.is_zero()
            && config.filter_subject == SNAPSHOT_PUBLISHED;
        if !compatible {
            return Err(TagRecoveryError::IncompatibleConsumer);
        }
        Ok(Self { consumer, nats })
    }

    pub async fn run(
        self: Arc<Self>,
        tag_store: Arc<dyn SnapshotTagStore>,
        cancel: CancellationToken,
    ) -> Result<(), TagRecoveryError> {
        let mut messages = self.consumer.stream().max_messages_per_batch(1).messages().await?;
        loop {
            let message = tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                next = messages.next() => match next {
                    Some(message) => message?,
                    None => return Err(TagRecoveryError::Closed),
                },
            };
            let published = match serde_json::from_slice::<SnapshotPublished>(&message.payload) {
                Ok(published) => published,
                Err(error) => {
                    warn!(%error, "discarding malformed snapshot tag recovery event");
                    message.double_ack().await.map_err(TagRecoveryError::Acknowledge)?;
                    continue;
                }
            };
            let result = tokio::select! {
                _ = cancel.cancelled() => {
                    message
                        .double_ack_with(AckKind::Nak(None))
                        .await
                        .map_err(TagRecoveryError::Acknowledge)?;
                    return Ok(());
                },
                result = recover(&self.nats, tag_store.as_ref(), &published) => result,
            };
            match result {
                Ok(()) => message.double_ack().await.map_err(TagRecoveryError::Acknowledge)?,
                Err(error) => {
                    warn!(lease_id = %published.lease_id.as_str(), %error, "snapshot tag recovery will retry");
                    message
                        .double_ack_with(AckKind::Nak(Some(TAG_RECOVERY_RETRY_DELAY)))
                        .await
                        .map_err(TagRecoveryError::Acknowledge)?;
                }
            }
        }
    }
}

async fn retry_tag(
    tag_store: &dyn SnapshotTagStore,
    published: &SnapshotPublished,
) -> Result<(), SnapshotTagStoreError> {
    let mut attempt = 0;
    loop {
        match auto_tag_published(tag_store, published).await {
            Ok(()) => return Ok(()),
            Err(error) => match TAG_RETRY_BACKOFF.get(attempt) {
                Some(delay) => {
                    tokio::time::sleep(*delay).await;
                    attempt += 1;
                }
                None => return Err(error),
            },
        }
    }
}

async fn recover(
    nats: &Client,
    tag_store: &dyn SnapshotTagStore,
    published: &SnapshotPublished,
) -> Result<(), RecoveryAttemptError> {
    let disposition = confirm_accounting(nats, published)
        .await
        .map_err(RecoveryAttemptError::Unconfirmed)?;
    if disposition == SnapshotAccountingDisposition::Live {
        retry_tag(tag_store, published).await?;
    }
    Ok(())
}
