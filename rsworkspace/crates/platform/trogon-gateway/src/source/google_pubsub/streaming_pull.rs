use std::collections::BTreeMap;
use std::error::Error;
use std::future::Future;
use std::pin::{Pin, pin};
use std::time::Duration;

use async_nats::HeaderMap;
use async_nats::header::{HeaderName, HeaderValue, NATS_MESSAGE_ID};
use bytes::Bytes;
use tracing::{info, instrument, warn};
use trogon_nats::jetstream::{ClaimCheckPublisher, JetStreamPublisher, ObjectStorePut};
use trogon_semconv::span::GOOGLE_PUBSUB_MESSAGE;

use super::config::{GooglePubSubConfig, GooglePubSubTransportConfig, StreamingPullConfig};
use super::constants::{
    NATS_HEADER_ATTRIBUTE_PREFIX, NATS_HEADER_MESSAGE_ID, NATS_HEADER_ORDERING_KEY, NATS_HEADER_PUBLISH_TIME,
    NATS_HEADER_SUBSCRIPTION, REDELIVERY_PAUSE, RESUBSCRIBE_INITIAL_DELAY, RESUBSCRIBE_MAX_DELAY, SOURCE_NAME,
    SUBJECT_SUFFIX_MESSAGE,
};
use super::pub_sub_subscription_name::PubSubSubscriptionName;

#[derive(Debug, thiserror::Error)]
pub enum PullError {
    #[error("failed to open Pub/Sub subscription stream: {0}")]
    Open(#[source] Box<dyn Error + Send + Sync>),
    #[error("Pub/Sub stream failed: {0}")]
    Stream(#[source] Box<dyn Error + Send + Sync>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PulledMessage {
    pub data: Bytes,
    pub message_id: String,
    pub publish_time: Option<String>,
    pub ordering_key: String,
    pub attributes: BTreeMap<String, String>,
}

pub trait MessageHandle: Send + 'static {
    fn ack(self);
    fn nack(self);
}

pub trait PullStream: Send {
    type Handle: MessageHandle;

    fn next(&mut self) -> impl Future<Output = Option<Result<(PulledMessage, Self::Handle), PullError>>> + Send;

    fn shutdown(&mut self) -> impl Future<Output = ()> + Send;
}

pub trait SubscriptionSource: Send {
    type Stream: PullStream;

    fn open<'a>(
        &'a mut self,
        config: &'a StreamingPullConfig,
    ) -> impl Future<Output = Result<Self::Stream, PullError>> + Send + 'a;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Delivery {
    Acked,
    Nacked,
}

enum SessionEnd {
    Shutdown,
    StreamEnded,
    Failed(PullError),
}

struct Session {
    delivered_any: bool,
    end: SessionEnd,
}

enum Pause {
    Elapsed,
    ShutdownRequested,
}

struct MessageSink<P: JetStreamPublisher, S: ObjectStorePut> {
    publisher: ClaimCheckPublisher<P, S>,
    subject: String,
    subscription: PubSubSubscriptionName,
    ack_timeout: Duration,
}

pub async fn run<P, S, Source, Shutdown>(
    publisher: ClaimCheckPublisher<P, S>,
    config: GooglePubSubConfig,
    mut source: Source,
    shutdown: Shutdown,
) where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    Source: SubscriptionSource,
    Shutdown: Future<Output = ()> + Send,
{
    let GooglePubSubTransportConfig::StreamingPull(streaming_pull) = config.transport;
    let sink = MessageSink {
        publisher,
        subject: format!("{}.{SUBJECT_SUFFIX_MESSAGE}", config.subject_prefix),
        subscription: streaming_pull.subscription.clone(),
        ack_timeout: config.nats_ack_timeout.into(),
    };
    let mut shutdown = pin!(shutdown);
    let mut resubscribe_delay = RESUBSCRIBE_INITIAL_DELAY;

    loop {
        let opened = tokio::select! {
            biased;
            () = shutdown.as_mut() => break,
            opened = source.open(&streaming_pull) => opened,
        };

        let session = match opened {
            Ok(mut stream) => {
                info!(subscription = %sink.subscription, "Pub/Sub streaming pull started");
                pull_session(&sink, &mut stream, &mut shutdown).await
            }
            Err(error) => Session {
                delivered_any: false,
                end: SessionEnd::Failed(error),
            },
        };

        match session.end {
            SessionEnd::Shutdown => break,
            SessionEnd::StreamEnded => {
                warn!(subscription = %sink.subscription, "Pub/Sub stream ended, resubscribing");
            }
            SessionEnd::Failed(error) => {
                warn!(subscription = %sink.subscription, error = %error, "Pub/Sub stream failed, resubscribing");
            }
        }

        if session.delivered_any {
            resubscribe_delay = RESUBSCRIBE_INITIAL_DELAY;
        }
        if matches!(pause(resubscribe_delay, &mut shutdown).await, Pause::ShutdownRequested) {
            break;
        }
        resubscribe_delay = next_resubscribe_delay(resubscribe_delay);
    }

    info!(subscription = %sink.subscription, "Pub/Sub streaming pull stopped");
}

fn next_resubscribe_delay(current: Duration) -> Duration {
    current.saturating_mul(2).min(RESUBSCRIBE_MAX_DELAY)
}

async fn pause<Shutdown: Future<Output = ()>>(duration: Duration, shutdown: &mut Pin<&mut Shutdown>) -> Pause {
    tokio::select! {
        biased;
        () = shutdown.as_mut() => Pause::ShutdownRequested,
        () = tokio::time::sleep(duration) => Pause::Elapsed,
    }
}

async fn pull_session<P, S, Stream, Shutdown>(
    sink: &MessageSink<P, S>,
    stream: &mut Stream,
    shutdown: &mut Pin<&mut Shutdown>,
) -> Session
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    Stream: PullStream,
    Shutdown: Future<Output = ()>,
{
    let mut delivered_any = false;
    let end = loop {
        let next = tokio::select! {
            biased;
            () = shutdown.as_mut() => break SessionEnd::Shutdown,
            next = stream.next() => next,
        };

        match next {
            None => break SessionEnd::StreamEnded,
            Some(Err(error)) => break SessionEnd::Failed(error),
            Some(Ok((message, handle))) => {
                delivered_any = true;
                if sink.handle_message(message, handle).await == Delivery::Nacked
                    && matches!(pause(REDELIVERY_PAUSE, shutdown).await, Pause::ShutdownRequested)
                {
                    break SessionEnd::Shutdown;
                }
            }
        }
    };
    stream.shutdown().await;
    Session { delivered_any, end }
}

impl<P: JetStreamPublisher, S: ObjectStorePut> MessageSink<P, S> {
    #[instrument(
        name = GOOGLE_PUBSUB_MESSAGE,
        skip_all,
        fields(
            message_id = %message.message_id,
            subject = %self.subject,
        )
    )]
    async fn handle_message<Handle: MessageHandle>(&self, message: PulledMessage, handle: Handle) -> Delivery {
        let headers = message_headers(&message, &self.subscription);
        let outcome = self
            .publisher
            .publish_event(self.subject.clone(), headers, message.data, self.ack_timeout)
            .await;

        if outcome.is_ok() {
            handle.ack();
            Delivery::Acked
        } else {
            outcome.log_on_error(SOURCE_NAME);
            handle.nack();
            Delivery::Nacked
        }
    }
}

fn message_headers(message: &PulledMessage, subscription: &PubSubSubscriptionName) -> HeaderMap {
    let mut headers = HeaderMap::new();

    if !message.message_id.is_empty() {
        insert_header(&mut headers, NATS_MESSAGE_ID.as_ref(), &message.message_id);
        insert_header(&mut headers, NATS_HEADER_MESSAGE_ID, &message.message_id);
    }
    if let Some(publish_time) = &message.publish_time {
        insert_header(&mut headers, NATS_HEADER_PUBLISH_TIME, publish_time);
    }
    if !message.ordering_key.is_empty() {
        insert_header(&mut headers, NATS_HEADER_ORDERING_KEY, &message.ordering_key);
    }
    insert_header(&mut headers, NATS_HEADER_SUBSCRIPTION, subscription.as_str());

    for (key, value) in &message.attributes {
        if is_header_token(key) {
            insert_header(&mut headers, &format!("{NATS_HEADER_ATTRIBUTE_PREFIX}{key}"), value);
        } else {
            warn!(attribute = ?key, "skipping Pub/Sub attribute whose key is not a valid NATS header name");
        }
    }

    headers
}

fn insert_header(headers: &mut HeaderMap, name: &str, value: &str) {
    match (HeaderName::try_from(name), value.parse::<HeaderValue>()) {
        (Ok(name), Ok(value)) => headers.insert(name, value),
        _ => warn!(header = ?name, "skipping Pub/Sub header that is not valid for NATS"),
    }
}

fn is_header_token(candidate: &str) -> bool {
    !candidate.is_empty()
        && candidate
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c))
}

#[cfg(test)]
mod tests;
