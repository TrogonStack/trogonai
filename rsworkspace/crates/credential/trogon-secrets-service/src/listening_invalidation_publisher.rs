use std::future::IntoFuture as _;

use async_nats::HeaderMap;
use async_nats::header::NATS_MESSAGE_ID;
use async_nats::jetstream::message::AckKind;
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;
use trogon_nats::jetstream::{JetStreamConsumer, JetStreamCreateConsumer, JetStreamPublisher};

use super::constants::{INVALIDATION_PUBLISHER_DURABLE, INVALIDATION_RETRY_DELAY};
use super::{
    CredentialEventDelivery, InvalidationNotice, InvalidationOutcome, InvalidationPublisherError,
    InvalidationSourceError, OwnerTenants, PublisherError, StoredCredentialEvent, error_chain,
};

/// An [`InvalidationPublisher`](super::InvalidationPublisher) whose durable
/// consumer exists and delivers, so every credential event appended since it
/// started listening reaches it.
///
/// The service has its publisher listening before it serves a release, so no
/// revocation of a value it released is appended before the durable that
/// announces it exists (ADR#0074).
pub struct ListeningInvalidationPublisher<S, P, O>
where
    S: JetStreamCreateConsumer,
{
    notices: P,
    owners: O,
    deliveries: <S::Consumer as JetStreamConsumer>::Messages,
}

impl<S, P, O> ListeningInvalidationPublisher<S, P, O>
where
    S: JetStreamCreateConsumer,
    <S::Consumer as JetStreamConsumer>::Message: CredentialEventDelivery,
    P: JetStreamPublisher,
    O: OwnerTenants,
{
    pub(crate) fn new(notices: P, owners: O, deliveries: <S::Consumer as JetStreamConsumer>::Messages) -> Self {
        Self {
            notices,
            owners,
            deliveries,
        }
    }

    /// Publishes the notice of each credential event until `shutdown` is
    /// cancelled. The event in hand when it is cancelled is settled first,
    /// so its notice is not left stored but unacknowledged until the stream
    /// delivers it again.
    ///
    /// A delivery that fails, or a settlement the credential stream does not
    /// take, is logged and reading goes on: the stream delivers an unsettled
    /// event again on its own.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidationPublisherError`] when the deliveries end before
    /// `shutdown`.
    pub async fn run(self, shutdown: CancellationToken) -> Result<(), PublisherError<S>> {
        let Self {
            notices,
            owners,
            mut deliveries,
        } = self;
        loop {
            let delivery = tokio::select! {
                biased;
                () = shutdown.cancelled() => return Ok(()),
                delivery = deliveries.next() => delivery,
            };
            match delivery {
                Some(Ok(delivery)) => settle(&notices, &owners, &delivery).await,
                Some(Err(error)) => {
                    tracing::warn!(error = %error, "credential event delivery failed; reading on");
                }
                None => {
                    tracing::error!(
                        durable = INVALIDATION_PUBLISHER_DURABLE,
                        "credential event deliveries ended before shutdown"
                    );
                    return Err(InvalidationPublisherError::MessagesEnded);
                }
            }
        }
    }
}

async fn settle(
    notices: &impl JetStreamPublisher,
    owners: &impl OwnerTenants,
    delivery: &impl CredentialEventDelivery,
) {
    let StoredCredentialEvent {
        stream: source_stream,
        message,
    } = match delivery.stored() {
        Ok(stored) => stored,
        Err(error) => {
            let error = error_chain(&error);
            tracing::error!(
                error = %error,
                "credential event delivery does not say where it was stored; retrying it"
            );
            retry(delivery, None).await;
            return;
        }
    };
    let sequence = message.sequence;
    match InvalidationOutcome::of(&source_stream, message, owners) {
        Ok(InvalidationOutcome::Publish(notice)) => publish(notices, delivery, sequence, &notice).await,
        Ok(InvalidationOutcome::NothingToPublish) => acknowledge(delivery, sequence).await,
        Ok(InvalidationOutcome::OwnerHeldByNoTenant { credential }) => {
            let credential_id = credential.credential_id();
            let owner_id = credential.owner_id();
            tracing::warn!(
                stream_sequence = sequence,
                credential_id = %credential_id,
                owner_id = %owner_id,
                "credential event ends a release whose owner no tenant holds; no notice published"
            );
            acknowledge(delivery, sequence).await;
        }
        Err(error) if retried(&error) => {
            let error = error_chain(&error);
            tracing::warn!(
                stream_sequence = sequence,
                error = %error,
                "credential event is not readable by this build yet; retrying it"
            );
            retry(delivery, Some(sequence)).await;
        }
        Err(error) => {
            let error = error_chain(&error);
            tracing::error!(
                stream_sequence = sequence,
                error = %error,
                "credential event is not readable; terminated without a notice"
            );
            settle_with(delivery, Some(sequence), AckKind::Term).await;
        }
    }
}

async fn publish(
    notices: &impl JetStreamPublisher,
    delivery: &impl CredentialEventDelivery,
    sequence: u64,
    notice: &InvalidationNotice,
) {
    let mut headers = HeaderMap::new();
    headers.insert(NATS_MESSAGE_ID, notice.message_id());
    let stored = match notices
        .publish_with_headers(notice.subject().to_owned(), headers, notice.payload().to_vec().into())
        .await
    {
        Ok(confirmation) => confirmation.into_future().await.map(drop),
        Err(error) => Err(error),
    };
    match stored {
        Ok(()) => acknowledge(delivery, sequence).await,
        Err(error) => {
            let subject = notice.subject();
            let error = error_chain(&error);
            tracing::warn!(
                stream_sequence = sequence,
                subject,
                error = %error,
                "invalidation notice was not stored; retrying its credential event"
            );
            retry(delivery, Some(sequence)).await;
        }
    }
}

/// Whether a credential event this build cannot read is delivered again
/// rather than terminated. Only an unknown event type can read differently on
/// a later delivery, by reaching a newer build; every other failure is in the
/// stored event itself.
pub(crate) fn retried(error: &InvalidationSourceError) -> bool {
    match error {
        InvalidationSourceError::UnknownEventType { .. } => true,
        InvalidationSourceError::Unrecorded(_)
        | InvalidationSourceError::Undecodable(_)
        | InvalidationSourceError::InvalidEvent(_)
        | InvalidationSourceError::NoSubject(_)
        | InvalidationSourceError::EventFromAnotherSubject { .. }
        | InvalidationSourceError::NotAppendedAfterEpoch { .. } => false,
    }
}

async fn acknowledge(delivery: &impl CredentialEventDelivery, sequence: u64) {
    if let Err(error) = delivery.ack().await {
        tracing::warn!(
            stream_sequence = sequence,
            error = %error,
            "credential event was not acknowledged; the stream delivers it again"
        );
    }
}

async fn retry(delivery: &impl CredentialEventDelivery, sequence: Option<u64>) {
    settle_with(delivery, sequence, AckKind::Nak(Some(INVALIDATION_RETRY_DELAY))).await;
}

async fn settle_with(delivery: &impl CredentialEventDelivery, sequence: Option<u64>, kind: AckKind) {
    if let Err(error) = delivery.ack_with(kind).await {
        tracing::warn!(
            stream_sequence = sequence,
            settlement = ?kind,
            error = %error,
            "credential event was not settled; the stream delivers it again"
        );
    }
}
