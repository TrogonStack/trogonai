use google_cloud_auth::credentials::Credentials;
use google_cloud_pubsub::client::Subscriber;
use google_cloud_pubsub::model::Message;
use google_cloud_pubsub::subscriber::handler::Handler;
use google_cloud_pubsub::subscriber::{MessageStream, ShutdownBehavior};

use super::config::StreamingPullConfig;
use super::streaming_pull::{MessageHandle, PullError, PullStream, PulledMessage, SubscriptionSource};

impl From<Message> for PulledMessage {
    fn from(message: Message) -> Self {
        Self {
            data: message.data,
            message_id: message.message_id,
            publish_time: message.publish_time.map(String::from),
            ordering_key: message.ordering_key,
            attributes: message.attributes.into_iter().collect(),
        }
    }
}

impl MessageHandle for Handler {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn ack(self) {
        Handler::ack(self);
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    fn nack(self) {
        Handler::nack(self);
    }
}

pub struct GooglePubSubSource {
    credentials: Credentials,
    subscriber: Option<Subscriber>,
}

impl GooglePubSubSource {
    pub fn new(credentials: Credentials) -> Self {
        Self {
            credentials,
            subscriber: None,
        }
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn subscriber(&mut self) -> Result<Subscriber, PullError> {
        if let Some(subscriber) = &self.subscriber {
            return Ok(subscriber.clone());
        }
        let subscriber = Subscriber::builder()
            .with_credentials(self.credentials.clone())
            .build()
            .await
            .map_err(|error| PullError::Open(Box::new(error)))?;
        self.subscriber = Some(subscriber.clone());
        Ok(subscriber)
    }
}

impl SubscriptionSource for GooglePubSubSource {
    type Stream = GooglePullStream;

    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn open<'a>(&'a mut self, config: &'a StreamingPullConfig) -> Result<Self::Stream, PullError> {
        let subscriber = self.subscriber().await?;
        let mut subscribe = subscriber
            .subscribe(config.subscription.as_str())
            .set_shutdown_behavior(ShutdownBehavior::NackImmediately);
        if let Some(max_outstanding_messages) = config.max_outstanding_messages {
            subscribe = subscribe.set_max_outstanding_messages(i64::from(max_outstanding_messages.get()));
        }
        Ok(GooglePullStream {
            stream: subscribe.build(),
        })
    }
}

pub struct GooglePullStream {
    stream: MessageStream,
}

impl PullStream for GooglePullStream {
    type Handle = Handler;

    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn next(&mut self) -> Option<Result<(PulledMessage, Self::Handle), PullError>> {
        let next = self.stream.next().await?;
        Some(
            next.map(|(message, handle)| (PulledMessage::from(message), handle))
                .map_err(|error| PullError::Stream(Box::new(error))),
        )
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn shutdown(&mut self) {
        self.stream.shutdown_token().shutdown().await;
    }
}

#[cfg(test)]
mod tests;
