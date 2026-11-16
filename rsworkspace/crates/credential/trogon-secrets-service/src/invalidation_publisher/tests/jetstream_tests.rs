//! The publisher against a real NATS JetStream server: the notices it stores
//! for credential events as the store appends them, and what a redelivery or
//! a restart does to them.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_nats::HeaderMap;
use async_nats::header::NATS_MESSAGE_ID;
use async_nats::jetstream::consumer::pull::MessagesError;
use async_nats::jetstream::consumer::{Consumer, DeliverPolicy, StreamError, pull};
use async_nats::jetstream::message::{AckKind, StreamMessage};
use async_nats::jetstream::stream::{self, ConsumerError, Stream};
use async_nats::jetstream::{Context, Message};
use async_nats::subject::ToSubject;
use buffa::Message as _;
use bytes::Bytes;
use futures::stream::{BoxStream, StreamExt as _};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use trogon_credential_aggregate::{CREDENTIAL_EVENT_SUBJECT_PREFIX, CREDENTIAL_STREAM};
use trogon_credential_domain::{CredentialIdentity, CredentialKind, CredentialOwnerId, CredentialScope, SourceKind};
use trogon_nats::jetstream::{
    JetStreamConsumer, JetStreamCreateConsumer, JetStreamPublishMessage as _, JetStreamPublisher, JsAck, JsAckWith,
};
use trogon_nats::test_support::JetStreamTestServer;
use trogonai_proto::gateway::credentials::v1;
use trogonai_proto::secrets::exchange::v1alpha1::{CredentialAddress, ReleaseInvalidated, ReleaseInvalidationReason};

use super::{HOLDER, holding_owner};
use crate::constants::INVALIDATION_PUBLISHER_DURABLE;
use crate::fixtures::credential_events::{activated, message, revoked, rotated};
use crate::fixtures::{OWNER, identity};
use crate::{
    CredentialEventDelivery, DeliveryInfoError, InvalidationPublisher, InvalidationPublisherError, InvalidationStream,
};

const SETTLED_WITHIN: Duration = Duration::from_secs(10);
const POLL_EVERY: Duration = Duration::from_millis(20);
const REVOKED: ReleaseInvalidationReason = ReleaseInvalidationReason::RELEASE_INVALIDATION_REASON_REVOKED;
const ROTATED: ReleaseInvalidationReason = ReleaseInvalidationReason::RELEASE_INVALIDATION_REASON_ROTATED;

type Running<E> = JoinHandle<Result<(), InvalidationPublisherError<E, StreamError>>>;

/// Another credential of [`OWNER`], so its notice goes to the same holder on
/// a subject of its own.
fn credential_of(kind: CredentialKind) -> CredentialIdentity {
    CredentialIdentity::derive(
        CredentialScope::source(CredentialOwnerId::new(OWNER).unwrap(), SourceKind::GitHub),
        kind,
    )
}

/// The credential stream and the invalidation stream on a fresh server.
struct Streams {
    _server: JetStreamTestServer,
    jetstream: Context,
    events: Stream,
    notices: Stream,
}

impl Streams {
    async fn start() -> Self {
        let server = JetStreamTestServer::start().await;
        let jetstream = server.jetstream().await;
        let events = jetstream
            .create_stream(stream::Config {
                name: CREDENTIAL_STREAM.to_owned(),
                subjects: vec![format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.>")],
                ..Default::default()
            })
            .await
            .unwrap();
        let notices = jetstream
            .create_stream(InvalidationStream::config(NonZeroUsize::MIN))
            .await
            .unwrap();
        Self {
            _server: server,
            jetstream,
            events,
            notices,
        }
    }

    /// Starts a publisher on the real streams and waits until it reads.
    async fn publishing(&self) -> (CancellationToken, Running<ConsumerError>) {
        let shutdown = CancellationToken::new();
        let running = tokio::spawn(
            InvalidationPublisher::new(self.events.clone(), self.jetstream.clone(), holding_owner())
                .run(shutdown.clone()),
        );
        self.reading().await;
        (shutdown, running)
    }

    /// Waits until the publisher's durable exists, since it reads only the
    /// events appended after that.
    async fn reading(&self) {
        tokio::time::timeout(SETTLED_WITHIN, async {
            while self.events.consumer_info(INVALIDATION_PUBLISHER_DURABLE).await.is_err() {
                tokio::time::sleep(POLL_EVERY).await;
            }
        })
        .await
        .expect("the publisher never created its durable");
    }

    /// Appends `event` as the credential store would, returning where the
    /// stream stored it.
    async fn append(&self, identity: &CredentialIdentity, event: &v1::CredentialEvent) -> u64 {
        self.jetstream
            .publish_message(message(identity, event))
            .await
            .unwrap()
            .await
            .unwrap()
            .sequence
    }

    /// Waits until the publisher has settled every event up to `sequence`.
    async fn settled(&self, sequence: u64) {
        tokio::time::timeout(SETTLED_WITHIN, async {
            loop {
                let durable = self.events.consumer_info(INVALIDATION_PUBLISHER_DURABLE).await.unwrap();
                if durable.ack_floor.stream_sequence >= sequence {
                    return;
                }
                tokio::time::sleep(POLL_EVERY).await;
            }
        })
        .await
        .expect("the publisher did not settle every event");
    }

    async fn notice_count(&self) -> u64 {
        self.notices.get_info().await.unwrap().state.messages
    }

    /// Checks the notice on `identity`'s subject tells its holder to drop the
    /// credential for `reason`, names the event stored at `sequence`, and
    /// carries nothing else.
    async fn assert_announces(&self, identity: &CredentialIdentity, reason: ReleaseInvalidationReason, sequence: u64) {
        let subject = format!("secrets.invalidation.v1.{}.{OWNER}.{}", *HOLDER, identity.id());
        let notice = self
            .notices
            .get_last_raw_message_by_subject(&subject)
            .await
            .expect("no notice on the holder's subject");
        let event = self.events.get_raw_message(sequence).await.unwrap();

        assert_eq!(
            notice.headers.get(NATS_MESSAGE_ID).map(|id| id.as_str()),
            Some(format!("{CREDENTIAL_STREAM}:{sequence}").as_str())
        );
        // Destructured without `..`, so a field added to the contract stops
        // this test compiling until someone decides whether it could hold a
        // value.
        let ReleaseInvalidated {
            credential,
            reason: sent,
            occurred_at,
            source_sequence,
        } = ReleaseInvalidated::decode_from_slice(&notice.payload).unwrap();
        let CredentialAddress {
            credential_id,
            owner_id,
        } = credential.into_option().unwrap();
        let occurred_at = occurred_at.into_option().unwrap();
        assert_eq!(credential_id, identity.id().to_string());
        assert_eq!(owner_id, OWNER);
        assert_eq!(sent.as_known(), Some(reason));
        assert_eq!(source_sequence, sequence);
        assert_eq!(
            (occurred_at.seconds, occurred_at.nanos),
            (
                event.time.unix_timestamp(),
                i32::try_from(event.time.nanosecond()).unwrap()
            )
        );
    }
}

/// Stops a running publisher and checks it stopped cleanly.
async fn stop<E: std::fmt::Debug>(shutdown: CancellationToken, running: Running<E>) {
    shutdown.cancel();
    tokio::time::timeout(SETTLED_WITHIN, running)
        .await
        .expect("the publisher did not stop when told to")
        .unwrap()
        .unwrap();
}

/// The invalidation stream, counting every notice handed to it.
#[derive(Clone)]
struct CountingNotices {
    jetstream: Context,
    published: Arc<AtomicUsize>,
}

impl CountingNotices {
    fn new(jetstream: Context) -> Self {
        Self {
            jetstream,
            published: Arc::default(),
        }
    }

    fn published(&self) -> usize {
        self.published.load(Ordering::SeqCst)
    }
}

impl JetStreamPublisher for CountingNotices {
    type PublishError = <Context as JetStreamPublisher>::PublishError;
    type AckFuture = <Context as JetStreamPublisher>::AckFuture;

    async fn publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> Result<Self::AckFuture, Self::PublishError> {
        self.published.fetch_add(1, Ordering::SeqCst);
        JetStreamPublisher::publish_with_headers(&self.jetstream, subject, headers, payload).await
    }
}

/// The credential stream, except the first event the publisher acknowledges
/// is handed back for immediate redelivery, as if the publisher had stopped
/// after storing its notice and before acknowledging it.
struct RedeliverFirst {
    events: Stream,
    redelivered: Arc<AtomicBool>,
}

struct RedeliverFirstConsumer {
    consumer: Consumer<pull::Config>,
    redelivered: Arc<AtomicBool>,
}

struct RedeliverFirstDelivery {
    delivery: Message,
    redelivered: Arc<AtomicBool>,
}

impl JetStreamCreateConsumer for RedeliverFirst {
    type Error = ConsumerError;
    type Consumer = RedeliverFirstConsumer;

    async fn create_consumer(&self, config: pull::Config) -> Result<Self::Consumer, Self::Error> {
        Ok(RedeliverFirstConsumer {
            consumer: self.events.create_consumer(config).await?,
            redelivered: Arc::clone(&self.redelivered),
        })
    }
}

impl JetStreamConsumer for RedeliverFirstConsumer {
    type StreamError = StreamError;
    type MessagesError = MessagesError;
    type Message = RedeliverFirstDelivery;
    type Messages = BoxStream<'static, Result<RedeliverFirstDelivery, MessagesError>>;

    async fn messages(&self) -> Result<Self::Messages, Self::StreamError> {
        let redelivered = Arc::clone(&self.redelivered);
        Ok(self
            .consumer
            .messages()
            .await?
            .map(move |delivery| {
                delivery.map(|delivery| RedeliverFirstDelivery {
                    delivery,
                    redelivered: Arc::clone(&redelivered),
                })
            })
            .boxed())
    }
}

impl JsAck for RedeliverFirstDelivery {
    type Error = async_nats::Error;

    async fn ack(&self) -> Result<(), Self::Error> {
        if self.redelivered.swap(true, Ordering::SeqCst) {
            self.delivery.ack().await
        } else {
            self.delivery.ack_with(AckKind::Nak(None)).await
        }
    }
}

impl JsAckWith for RedeliverFirstDelivery {
    type Error = async_nats::Error;

    async fn ack_with(&self, kind: AckKind) -> Result<(), Self::Error> {
        self.delivery.ack_with(kind).await
    }
}

impl CredentialEventDelivery for RedeliverFirstDelivery {
    fn stored(&self) -> Result<StreamMessage, DeliveryInfoError> {
        self.delivery.stored()
    }
}

/// The credential stream delivered again from its first event, as it is once
/// an operator recreates the durable to replay the stream.
struct ReplayFromStart(Stream);

impl JetStreamCreateConsumer for ReplayFromStart {
    type Error = ConsumerError;
    type Consumer = Consumer<pull::Config>;

    async fn create_consumer(&self, config: pull::Config) -> Result<Self::Consumer, Self::Error> {
        self.0
            .create_consumer(pull::Config {
                deliver_policy: DeliverPolicy::All,
                ..config
            })
            .await
    }
}

#[tokio::test]
async fn an_event_that_leaves_releases_as_they_were_stores_no_notice() {
    let streams = Streams::start().await;
    let (shutdown, running) = streams.publishing().await;

    let activated_at = streams.append(&identity(), &activated(&identity())).await;
    streams.settled(activated_at).await;
    stop(shutdown, running).await;

    assert_eq!(streams.notice_count().await, 0);
}

#[tokio::test]
async fn a_revocation_delivered_again_after_its_notice_was_stored_stores_one_notice() {
    let streams = Streams::start().await;
    let notices = CountingNotices::new(streams.jetstream.clone());
    let shutdown = CancellationToken::new();
    let events = RedeliverFirst {
        events: streams.events.clone(),
        redelivered: Arc::default(),
    };
    let running =
        tokio::spawn(InvalidationPublisher::new(events, notices.clone(), holding_owner()).run(shutdown.clone()));
    streams.reading().await;

    let revoked_at = streams.append(&identity(), &revoked(&identity())).await;
    streams.settled(revoked_at).await;
    stop(shutdown, running).await;

    assert_eq!(notices.published(), 2, "the revocation was not delivered again");
    assert_eq!(streams.notice_count().await, 1);
    streams.assert_announces(&identity(), REVOKED, revoked_at).await;
}

#[tokio::test]
async fn a_revocation_replayed_from_the_start_of_the_credential_stream_stores_one_notice() {
    let streams = Streams::start().await;
    let (shutdown, running) = streams.publishing().await;
    let revoked_at = streams.append(&identity(), &revoked(&identity())).await;
    streams.settled(revoked_at).await;
    stop(shutdown, running).await;
    streams
        .events
        .delete_consumer(INVALIDATION_PUBLISHER_DURABLE)
        .await
        .unwrap();

    let notices = CountingNotices::new(streams.jetstream.clone());
    let shutdown = CancellationToken::new();
    let running = tokio::spawn(
        InvalidationPublisher::new(
            ReplayFromStart(streams.events.clone()),
            notices.clone(),
            holding_owner(),
        )
        .run(shutdown.clone()),
    );
    streams.reading().await;
    streams.settled(revoked_at).await;
    stop(shutdown, running).await;

    assert_eq!(
        notices.published(),
        1,
        "the replay did not publish the revocation again"
    );
    assert_eq!(streams.notice_count().await, 1);
    streams.assert_announces(&identity(), REVOKED, revoked_at).await;
}

#[tokio::test]
async fn a_revocation_appended_as_soon_as_the_publisher_listens_is_announced() {
    let streams = Streams::start().await;
    let shutdown = CancellationToken::new();
    let listening = InvalidationPublisher::new(streams.events.clone(), streams.jetstream.clone(), holding_owner())
        .listen(&shutdown)
        .await
        .unwrap()
        .expect("listening before shutdown");
    let revocation = streams.append(&identity(), &revoked(&identity())).await;

    let running = tokio::spawn(listening.run(shutdown.clone()));
    streams.settled(revocation).await;
    stop(shutdown, running).await;

    streams.assert_announces(&identity(), REVOKED, revocation).await;
}

#[tokio::test]
async fn a_restarted_publisher_resumes_from_its_durable_without_republishing_what_it_acknowledged() {
    let streams = Streams::start().await;
    let (shutdown, running) = streams.publishing().await;
    let first = streams.append(&identity(), &revoked(&identity())).await;
    streams.settled(first).await;
    stop(shutdown, running).await;
    let missed_credential = credential_of(CredentialKind::SigningSecret);
    let missed = streams.append(&missed_credential, &revoked(&missed_credential)).await;

    let notices = CountingNotices::new(streams.jetstream.clone());
    let shutdown = CancellationToken::new();
    let running = tokio::spawn(
        InvalidationPublisher::new(streams.events.clone(), notices.clone(), holding_owner()).run(shutdown.clone()),
    );
    streams.settled(missed).await;
    stop(shutdown, running).await;

    assert_eq!(
        notices.published(),
        1,
        "the restarted publisher published an event it had acknowledged, or missed one"
    );
    assert_eq!(streams.notice_count().await, 2);
    streams.assert_announces(&identity(), REVOKED, first).await;
    streams.assert_announces(&missed_credential, REVOKED, missed).await;
}

#[tokio::test]
async fn a_revocation_and_a_rotation_each_reach_their_holder_as_one_notice() {
    let streams = Streams::start().await;
    let (shutdown, running) = streams.publishing().await;
    let rotating = credential_of(CredentialKind::SigningSecret);

    let revoked_at = streams.append(&identity(), &revoked(&identity())).await;
    let rotated_at = streams.append(&rotating, &rotated(&rotating)).await;
    streams.settled(rotated_at).await;
    stop(shutdown, running).await;

    assert_eq!(streams.notice_count().await, 2);
    streams.assert_announces(&identity(), REVOKED, revoked_at).await;
    streams.assert_announces(&rotating, ROTATED, rotated_at).await;
}
