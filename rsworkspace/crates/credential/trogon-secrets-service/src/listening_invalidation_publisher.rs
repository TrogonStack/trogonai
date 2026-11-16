use tokio_util::sync::CancellationToken;
use trogon_nats::jetstream::{JetStreamConsumer, JetStreamCreateConsumer, JetStreamPublisher};

use super::invalidation_publisher::PublisherError;
use super::{CredentialEventDelivery, InvalidationPublisher, OwnerTenants};

/// An [`InvalidationPublisher`] whose durable consumer exists and delivers, so
/// every credential event appended since it started listening reaches it.
///
/// The service has its publisher listening before it serves a release, so no
/// revocation of a value it released is appended before the durable that
/// announces it exists (ADR#0074).
pub struct ListeningInvalidationPublisher<S, P, O>
where
    S: JetStreamCreateConsumer,
{
    publisher: InvalidationPublisher<S, P, O>,
    deliveries: <S::Consumer as JetStreamConsumer>::Messages,
}

impl<S, P, O> ListeningInvalidationPublisher<S, P, O>
where
    S: JetStreamCreateConsumer,
    <S::Consumer as JetStreamConsumer>::Message: CredentialEventDelivery,
    P: JetStreamPublisher,
    O: OwnerTenants,
{
    pub(crate) fn new(
        publisher: InvalidationPublisher<S, P, O>,
        deliveries: <S::Consumer as JetStreamConsumer>::Messages,
    ) -> Self {
        Self { publisher, deliveries }
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
    /// Returns [`InvalidationPublisherError`](super::InvalidationPublisherError)
    /// when the deliveries end before `shutdown`.
    pub async fn run(self, shutdown: CancellationToken) -> Result<(), PublisherError<S>> {
        self.publisher.read(self.deliveries, shutdown).await
    }
}
