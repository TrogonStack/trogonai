use std::sync::Arc;

use async_nats::{Message, Subject};
use futures::StreamExt as _;
use grpc_nats_micro::constants::DEFAULT_QUEUE_GROUP;
use grpc_nats_micro::{EndpointHandler, answer};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use trogon_nats::{PublishClient, QueueSubscribeClient};
use trogonai_proto::nats::micro::v1alpha1::{ContentType as ProtoContentType, ServiceOptions};

use super::constants::RELEASE_SUBSCRIPTION;
use super::throttled_answer::ThrottledAnswer;
use super::{AdmissionLimit, ReleaseAnswer, ReleaseCredential, ReleaseServerError};

/// Serves `SecretExchangeService.ResolveForRelease` for every tenant's runtime
/// services as one member of the release queue group.
///
/// The endpoint is a queue subscription on [`RELEASE_SUBSCRIPTION`] rather than
/// a micro registration: the caller's tenant and runtime service are wildcard
/// tokens of the subject, which a micro endpoint subject cannot carry. Replies
/// still take the micro content type negotiation and error channel of
/// ADR#0016 through [`grpc_nats_micro::answer`], encoded as protobuf, the only
/// encoding the exchange contract declares.
///
/// At most [`AdmissionLimit`] releases are in flight at once; a call beyond
/// them is answered `THROTTLED` at once.
pub struct ReleaseServer<C, R> {
    client: C,
    release: Arc<R>,
    admission: Arc<Semaphore>,
    options: Arc<ServiceOptions>,
}

impl<C, R> ReleaseServer<C, R>
where
    C: QueueSubscribeClient + PublishClient,
    R: ReleaseCredential,
{
    pub fn new(client: C, release: Arc<R>, limit: AdmissionLimit) -> Self {
        Self {
            client,
            release,
            admission: Arc::new(Semaphore::new(limit.get())),
            options: Arc::new(ServiceOptions {
                content_type: ProtoContentType::CONTENT_TYPE_PROTOBUF.into(),
                ..Default::default()
            }),
        }
    }

    /// Joins the release queue group and answers calls until `shutdown` is
    /// cancelled, then leaves the group and waits for every release in flight
    /// to be answered.
    ///
    /// A subscription that ends before `shutdown` is an error once the
    /// releases in flight are answered, so the process can exit and be
    /// replaced rather than stay up answering nothing.
    pub async fn run(self, shutdown: CancellationToken) -> Result<(), ReleaseServerError<C::QueueSubscribeError>> {
        let mut subscription = self
            .client
            .queue_subscribe(
                Subject::from_static(RELEASE_SUBSCRIPTION),
                DEFAULT_QUEUE_GROUP.to_owned(),
            )
            .await
            .map_err(ReleaseServerError::Join)?;
        tracing::info!(
            subject = RELEASE_SUBSCRIPTION,
            queue_group = DEFAULT_QUEUE_GROUP,
            "release endpoint joined its queue group"
        );
        let in_flight = TaskTracker::new();
        let ended = loop {
            let message = tokio::select! {
                biased;
                () = shutdown.cancelled() => break Ok(()),
                message = subscription.next() => match message {
                    Some(message) => message,
                    None => break Err(ReleaseServerError::SubscriptionEnded),
                },
            };
            let Some(reply) = message.reply.clone() else {
                tracing::warn!(subject = %message.subject, "release call without a reply subject; dropped");
                continue;
            };
            let Ok(permit) = Arc::clone(&self.admission).try_acquire_owned() else {
                tracing::warn!(subject = %message.subject, "release endpoint at its admission limit; throttled");
                respond(&self.client, &self.options, reply, &message, &ThrottledAnswer).await;
                continue;
            };
            let client = self.client.clone();
            let release = Arc::clone(&self.release);
            let options = Arc::clone(&self.options);
            in_flight.spawn(async move {
                let _permit = permit;
                let handler = ReleaseAnswer::new(message.subject.as_str(), release.as_ref());
                respond(&client, &options, reply, &message, &handler).await;
            });
        };
        drop(subscription);
        in_flight.close();
        in_flight.wait().await;
        if ended.is_err() {
            tracing::error!(
                subject = RELEASE_SUBSCRIPTION,
                "release subscription ended before shutdown"
            );
        }
        ended
    }
}

async fn respond<C: PublishClient>(
    client: &C,
    options: &ServiceOptions,
    reply: Subject,
    message: &Message,
    handler: &dyn EndpointHandler,
) {
    let encoded = match answer(message.headers.as_ref(), &message.payload, options, handler).await {
        Ok(encoded) => encoded,
        Err(error) => {
            tracing::error!(subject = %message.subject, error = %error, "release reply did not encode; dropped");
            return;
        }
    };
    if let Err(error) = client.publish_with_headers(reply, encoded.headers, encoded.body).await {
        tracing::error!(subject = %message.subject, error = %error, "release reply was not published");
    }
}

#[cfg(test)]
mod tests;
