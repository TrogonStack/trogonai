use std::future::IntoFuture as _;

use async_nats::HeaderMap;
use async_nats::header::NATS_MESSAGE_ID;
use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, pull};
use async_nats::jetstream::message::AckKind;
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;
use trogon_credential_aggregate::CREDENTIAL_EVENT_SUBJECT_PREFIX;
use trogon_nats::jetstream::{JetStreamConsumer, JetStreamCreateConsumer, JetStreamPublisher};

use super::constants::{INVALIDATION_PUBLISHER_ACK_WAIT, INVALIDATION_PUBLISHER_DURABLE, INVALIDATION_RETRY_DELAY};
use super::{
    CredentialEventDelivery, InvalidationNotice, InvalidationOutcome, InvalidationPublisherError,
    InvalidationSourceError, ListeningInvalidationPublisher, OwnerTenants, error_chain,
};

pub(crate) type PublisherError<S> = InvalidationPublisherError<
    <S as JetStreamCreateConsumer>::Error,
    <<S as JetStreamCreateConsumer>::Consumer as JetStreamConsumer>::StreamError,
>;

/// Turns each credential event that ends a release into the notice telling its
/// holders to drop their copies (ADR#0074), stored on the invalidation stream.
///
/// It reads the credential stream through [`INVALIDATION_PUBLISHER_DURABLE`],
/// one durable pull consumer every member of the queue group shares, from the
/// events appended after it was first created. Each event is settled only once
/// the publisher is done with it, so one that is never settled is delivered
/// again, and a notice published twice carries the same `Nats-Msg-Id` and is
/// stored once.
///
/// | Credential event | Settlement |
/// | --- | --- |
/// | Its notice is stored | acknowledged, only once the invalidation stream confirms the notice |
/// | Its notice is refused or never confirmed | delivered again after a delay, never acknowledged |
/// | Leaves what the credential releases as it was | acknowledged |
/// | Ends a release whose owner no tenant holds | acknowledged, logged with the credential |
/// | Its type is unknown to this build | delivered again after a delay |
/// | Not a recorded event, undecodable, naming no credential, no subject token, on another credential's subject, or appended at the Unix epoch | terminated, logged as an error |
/// | The delivery does not say where it was stored | delivered again after a delay, logged as an error |
///
/// An unknown event type is retried because a newer build in the same rolling
/// deploy can read it, and it may be a revocation. The terminated events are
/// stored as they are, so no delivery reads them differently, and retrying one
/// would only redeliver it forever. A delivery without its stream position is
/// retried because the position comes from the client's reading of the
/// delivery, not from the event, and terminating on it would drop every
/// revocation alongside it.
///
/// Every log names the event by its stream sequence. Events are settled one at
/// a time, and none waits on another's retry, so an event that cannot be
/// published never holds back the revocations after it.
pub struct InvalidationPublisher<S, P, O> {
    events: S,
    notices: P,
    owners: O,
}

impl<S, P, O> InvalidationPublisher<S, P, O>
where
    S: JetStreamCreateConsumer,
    <S::Consumer as JetStreamConsumer>::Message: CredentialEventDelivery,
    P: JetStreamPublisher,
    O: OwnerTenants,
{
    /// A publisher reading credential events from `events`, storing notices
    /// through `notices`, and routing each notice to the tenant `owners` says
    /// holds the credential's owner.
    pub fn new(events: S, notices: P, owners: O) -> Self {
        Self {
            events,
            notices,
            owners,
        }
    }

    /// Listens, then publishes the notice of each credential event until
    /// `shutdown` is cancelled, as [`Self::listen`] and
    /// [`ListeningInvalidationPublisher::run`] do.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidationPublisherError`] when the durable consumer cannot
    /// be created, when it cannot start delivering, and when its deliveries
    /// end before `shutdown`.
    pub async fn run(self, shutdown: CancellationToken) -> Result<(), PublisherError<S>> {
        match self.listen(&shutdown).await? {
            Some(listening) => listening.run(shutdown).await,
            None => Ok(()),
        }
    }

    /// Creates the durable consumer, or finds it, and starts its deliveries,
    /// so every credential event appended once this returns reaches the
    /// publisher: the durable reads only the events appended after it was
    /// first created. Returns `None` when `shutdown` comes first.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidationPublisherError`] when the durable consumer cannot
    /// be created, or cannot start delivering.
    pub async fn listen(
        self,
        shutdown: &CancellationToken,
    ) -> Result<Option<ListeningInvalidationPublisher<S, P, O>>, PublisherError<S>> {
        let consumer = tokio::select! {
            biased;
            () = shutdown.cancelled() => return Ok(None),
            consumer = self.events.create_consumer(consumer_config()) => {
                consumer.map_err(InvalidationPublisherError::Consumer)?
            }
        };
        let deliveries = tokio::select! {
            biased;
            () = shutdown.cancelled() => return Ok(None),
            deliveries = consumer.messages() => deliveries.map_err(InvalidationPublisherError::Messages)?,
        };
        tracing::info!(
            durable = INVALIDATION_PUBLISHER_DURABLE,
            "invalidation publisher reading the credential stream"
        );
        Ok(Some(ListeningInvalidationPublisher::new(self, deliveries)))
    }

    /// Settles each of `deliveries` until `shutdown` is cancelled, as
    /// [`ListeningInvalidationPublisher::run`] says.
    pub(crate) async fn read(
        &self,
        mut deliveries: <S::Consumer as JetStreamConsumer>::Messages,
        shutdown: CancellationToken,
    ) -> Result<(), PublisherError<S>> {
        loop {
            let delivery = tokio::select! {
                biased;
                () = shutdown.cancelled() => return Ok(()),
                delivery = deliveries.next() => delivery,
            };
            match delivery {
                Some(Ok(delivery)) => self.settle(&delivery).await,
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

    async fn settle(&self, delivery: &impl CredentialEventDelivery) {
        let message = match delivery.stored() {
            Ok(message) => message,
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
        match InvalidationOutcome::of(message, &self.owners) {
            Ok(InvalidationOutcome::Publish(notice)) => self.publish(delivery, sequence, &notice).await,
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

    async fn publish(&self, delivery: &impl CredentialEventDelivery, sequence: u64, notice: &InvalidationNotice) {
        let mut headers = HeaderMap::new();
        headers.insert(NATS_MESSAGE_ID, notice.message_id());
        let stored = match self
            .notices
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
}

/// Whether a credential event this build cannot read is delivered again
/// rather than terminated. Only an unknown event type can read differently on
/// a later delivery, by reaching a newer build; every other failure is in the
/// stored event itself.
fn retried(error: &InvalidationSourceError) -> bool {
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

fn consumer_config() -> pull::Config {
    pull::Config {
        durable_name: Some(INVALIDATION_PUBLISHER_DURABLE.to_owned()),
        filter_subject: format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.>"),
        deliver_policy: DeliverPolicy::New,
        ack_policy: AckPolicy::Explicit,
        ack_wait: INVALIDATION_PUBLISHER_ACK_WAIT,
        ..Default::default()
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

#[cfg(test)]
mod tests;
