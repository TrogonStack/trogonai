use std::collections::VecDeque;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use async_nats::HeaderMap;
use async_nats::header::NATS_MESSAGE_ID;
use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, pull};
use async_nats::jetstream::message::{AckKind, OutboundMessage, StreamMessage};
use async_nats::jetstream::publish::PublishAck;
use async_nats::subject::ToSubject;
use bytes::Bytes;
use futures::channel::mpsc;
use futures::future::BoxFuture;
use futures::stream::{self, BoxStream, StreamExt as _};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use trogon_credential_aggregate::{CREDENTIAL_STREAM, CredentialEventSubjectResolverError};
use trogon_credential_domain::{CredentialIdentity, CredentialKind, CredentialOwnerId, CredentialScope, SourceKind};
use trogon_decider_runtime::{EventEncode, EventType};
use trogon_nats::jetstream::{JetStreamConsumer, JetStreamCreateConsumer, JetStreamPublisher, JsAck, JsAckWith};
use trogon_nats::mocks::MockError;
use trogon_std::log_capture::{CapturedEvent, CapturedEvents, LevelFilter};
use trogonai_proto::gateway::credentials::{CredentialEventCase, v1};

use super::InvalidationPublisher;
use crate::constants::{INVALIDATION_PUBLISHER_ACK_WAIT, INVALIDATION_PUBLISHER_DURABLE, INVALIDATION_RETRY_DELAY};
use crate::fixtures::credential_events::{activated, message, raw_message, revoked, rotated};
use crate::fixtures::{OWNER, identity, tenant};
use crate::listening_invalidation_publisher::retried;
use crate::{
    CredentialEventDelivery, DeliveryInfoError, InvalidationNotice, InvalidationOutcome, InvalidationPublisherError,
    InvalidationSourceError, StaticTenantOwners, StoredCredentialEvent, TenantAccount,
};

const APPENDED_SECONDS: i64 = 1_700_000_000;

/// One tenant for the whole module, so the notice a test expects and the one
/// the publisher builds route to the same account.
static HOLDER: LazyLock<TenantAccount> = LazyLock::new(tenant);

type Journal = Arc<Mutex<Vec<Step>>>;
type Deliveries = BoxStream<'static, Result<TestDelivery, MockError>>;
type PublisherError = InvalidationPublisherError<MockError, MockError>;

/// What the publisher did, in the order it did it, across the credential
/// stream and the invalidation stream.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    Published {
        subject: String,
        message_id: Option<String>,
        payload: Vec<u8>,
    },
    Stored,
    Acked(u64),
    Retried(u64, Option<Duration>),
    Terminated(u64),
}

fn published(notice: &InvalidationNotice) -> Step {
    Step::Published {
        subject: notice.subject().to_owned(),
        message_id: Some(notice.message_id().to_owned()),
        payload: notice.payload().to_vec(),
    }
}

fn retrying(sequence: u64) -> Step {
    Step::Retried(sequence, Some(INVALIDATION_RETRY_DELAY))
}

/// One credential event as the consumer would deliver it, settled into the
/// journal.
struct TestDelivery {
    sequence: u64,
    stored: Option<StreamMessage>,
    settles: bool,
    journal: Journal,
}

impl TestDelivery {
    fn settle(&self, step: Step) -> Result<(), MockError> {
        if !self.settles {
            return Err(MockError("fake-settle-refused".to_owned()));
        }
        self.journal.lock().unwrap().push(step);
        Ok(())
    }
}

impl JsAck for TestDelivery {
    type Error = MockError;

    async fn ack(&self) -> Result<(), Self::Error> {
        self.settle(Step::Acked(self.sequence))
    }
}

impl JsAckWith for TestDelivery {
    type Error = MockError;

    async fn ack_with(&self, kind: AckKind) -> Result<(), Self::Error> {
        let step = match kind {
            AckKind::Nak(delay) => Step::Retried(self.sequence, delay),
            AckKind::Term => Step::Terminated(self.sequence),
            other => panic!("the publisher settles only by ack, nak, or term, not {other:?}"),
        };
        self.settle(step)
    }
}

impl CredentialEventDelivery for TestDelivery {
    fn stored(&self) -> Result<StoredCredentialEvent, DeliveryInfoError> {
        self.stored
            .clone()
            .map(|message| StoredCredentialEvent {
                stream: CREDENTIAL_STREAM.to_owned(),
                message,
            })
            .ok_or_else(|| DeliveryInfoError(Box::new(MockError("fake-unparsable-reply".to_owned()))))
    }
}

/// The credential stream: hands out one consumer and remembers how it was
/// asked for.
#[derive(Clone)]
struct TestEvents {
    consumer: Arc<Mutex<Option<Result<TestConsumer, MockError>>>>,
    configs: Arc<Mutex<Vec<pull::Config>>>,
}

impl TestEvents {
    fn delivering(deliveries: Deliveries) -> Self {
        Self::answering(Ok(TestConsumer {
            messages: Mutex::new(Some(Ok(deliveries))),
        }))
    }

    fn answering(consumer: Result<TestConsumer, MockError>) -> Self {
        Self {
            consumer: Arc::new(Mutex::new(Some(consumer))),
            configs: Arc::default(),
        }
    }

    fn configs(&self) -> Vec<pull::Config> {
        self.configs.lock().unwrap().clone()
    }
}

impl JetStreamCreateConsumer for TestEvents {
    type Error = MockError;
    type Consumer = TestConsumer;

    async fn create_consumer(&self, config: pull::Config) -> Result<Self::Consumer, Self::Error> {
        self.configs.lock().unwrap().push(config);
        self.consumer.lock().unwrap().take().expect("one consumer per run")
    }
}

struct TestConsumer {
    messages: Mutex<Option<Result<Deliveries, MockError>>>,
}

impl JetStreamConsumer for TestConsumer {
    type StreamError = MockError;
    type MessagesError = MockError;
    type Message = TestDelivery;
    type Messages = Deliveries;

    async fn messages(&self) -> Result<Self::Messages, Self::StreamError> {
        self.messages
            .lock()
            .unwrap()
            .take()
            .expect("one delivery stream per run")
    }
}

/// How the invalidation stream answers one notice.
enum Answer {
    Stored,
    Refused,
    Unconfirmed,
    Held(oneshot::Receiver<()>),
}

/// The invalidation stream: answers each notice as scripted, storing it when
/// the script has run out.
#[derive(Clone)]
struct TestNotices {
    answers: Arc<Mutex<VecDeque<Answer>>>,
    journal: Journal,
}

impl TestNotices {
    fn new(journal: &Journal, answers: impl IntoIterator<Item = Answer>) -> Self {
        Self {
            answers: Arc::new(Mutex::new(answers.into_iter().collect())),
            journal: Arc::clone(journal),
        }
    }
}

impl JetStreamPublisher for TestNotices {
    type PublishError = MockError;
    type AckFuture = BoxFuture<'static, Result<PublishAck, MockError>>;

    async fn publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> Result<Self::AckFuture, Self::PublishError> {
        self.journal.lock().unwrap().push(Step::Published {
            subject: subject.to_subject().to_string(),
            message_id: headers.get(NATS_MESSAGE_ID).map(|id| id.as_str().to_owned()),
            payload: payload.to_vec(),
        });
        let answer = self.answers.lock().unwrap().pop_front().unwrap_or(Answer::Stored);
        let journal = Arc::clone(&self.journal);
        let stored = move || {
            journal.lock().unwrap().push(Step::Stored);
            Ok(PublishAck::default())
        };
        match answer {
            Answer::Refused => Err(MockError("fake-publish-refused".to_owned())),
            Answer::Stored => Ok(Box::pin(async move { stored() })),
            Answer::Unconfirmed => Ok(Box::pin(async { Err(MockError("fake-ack-timed-out".to_owned())) })),
            Answer::Held(release) => Ok(Box::pin(async move {
                release.await.unwrap();
                stored()
            })),
        }
    }
}

fn appended() -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp(APPENDED_SECONDS).unwrap()
}

/// `message` as the credential stream stores it at `sequence`.
fn stored(sequence: u64, message: OutboundMessage) -> StreamMessage {
    StreamMessage {
        subject: message.subject,
        sequence,
        headers: message.headers.unwrap_or_default(),
        payload: message.payload,
        time: appended(),
    }
}

fn holding_owner() -> StaticTenantOwners {
    StaticTenantOwners::new().with_owner(HOLDER.clone(), CredentialOwnerId::new(OWNER).unwrap())
}

fn other_identity() -> CredentialIdentity {
    CredentialIdentity::derive(
        CredentialScope::source(CredentialOwnerId::new("project-2").unwrap(), SourceKind::GitHub),
        CredentialKind::WebhookSecret,
    )
}

/// The notice the contract builds for `message`, which is what the publisher
/// must hand to the invalidation stream unchanged.
fn notice_for(message: &StreamMessage, owners: &StaticTenantOwners) -> InvalidationNotice {
    match InvalidationOutcome::of(CREDENTIAL_STREAM, message.clone(), owners) {
        Ok(InvalidationOutcome::Publish(notice)) => notice,
        other => panic!("expected a notice, got {other:?}"),
    }
}

struct Run {
    journal: Journal,
}

impl Run {
    fn new() -> Self {
        Self {
            journal: Arc::default(),
        }
    }

    fn delivery(&self, message: StreamMessage) -> TestDelivery {
        TestDelivery {
            sequence: message.sequence,
            stored: Some(message),
            settles: true,
            journal: Arc::clone(&self.journal),
        }
    }

    fn unreadable(&self, sequence: u64) -> TestDelivery {
        TestDelivery {
            sequence,
            stored: None,
            settles: true,
            journal: Arc::clone(&self.journal),
        }
    }

    fn unsettleable(&self, message: StreamMessage) -> TestDelivery {
        TestDelivery {
            settles: false,
            ..self.delivery(message)
        }
    }

    fn notices(&self, answers: impl IntoIterator<Item = Answer>) -> TestNotices {
        TestNotices::new(&self.journal, answers)
    }

    /// Publishes every delivery in order, with the notice stream answering as
    /// scripted, until the deliveries run out.
    async fn publish(
        &self,
        deliveries: impl IntoIterator<Item = Result<TestDelivery, MockError>>,
        answers: impl IntoIterator<Item = Answer>,
        owners: StaticTenantOwners,
    ) -> Result<(), PublisherError> {
        let deliveries: Vec<_> = deliveries.into_iter().collect();
        let events = TestEvents::delivering(stream::iter(deliveries).boxed());
        InvalidationPublisher::new(events, self.notices(answers), owners)
            .run(CancellationToken::new())
            .await
    }

    fn steps(&self) -> Vec<Step> {
        self.journal.lock().unwrap().clone()
    }
}

fn ran_out(outcome: Result<(), PublisherError>) {
    assert!(
        matches!(outcome, Err(InvalidationPublisherError::MessagesEnded)),
        "{outcome:?}"
    );
}

fn logged_at(events: &CapturedEvents, sequence: u64) -> Vec<CapturedEvent> {
    let sequence = sequence.to_string();
    events
        .events()
        .into_iter()
        .filter(|event| event.field("stream_sequence") == Some(sequence.as_str()))
        .collect()
}

#[tokio::test]
async fn a_notice_the_invalidation_stream_refuses_is_retried_and_its_event_never_acknowledged() {
    let run = Run::new();
    let revocation = stored(7, message(&identity(), &revoked(&identity())));
    let notice = notice_for(&revocation, &holding_owner());

    let outcome = run
        .publish([Ok(run.delivery(revocation))], [Answer::Refused], holding_owner())
        .await;

    ran_out(outcome);
    assert_eq!(run.steps(), [published(&notice), retrying(7)]);
}

#[tokio::test]
async fn a_notice_the_invalidation_stream_does_not_confirm_is_retried_and_its_event_never_acknowledged() {
    let run = Run::new();
    let revocation = stored(7, message(&identity(), &revoked(&identity())));
    let notice = notice_for(&revocation, &holding_owner());

    let outcome = run
        .publish([Ok(run.delivery(revocation))], [Answer::Unconfirmed], holding_owner())
        .await;

    ran_out(outcome);
    assert_eq!(run.steps(), [published(&notice), retrying(7)]);
}

/// A credential event the publisher cannot read, what it reads as, and
/// whether the publisher retries it rather than terminating it.
struct Unreadable {
    case: &'static str,
    message: StreamMessage,
    reads_as: fn(&InvalidationSourceError) -> bool,
    retries: bool,
}

/// Every way a stored message can fail to read that a message can carry; the
/// one it cannot, an owner with no subject token, is unrepresentable in an
/// owner id and is checked on its own.
fn unreadable_events() -> Vec<Unreadable> {
    let revocation = revoked(&identity());
    let event_type = revocation.event_type().unwrap();
    let mut at_epoch = stored(6, message(&identity(), &revocation));
    at_epoch.time = time::OffsetDateTime::UNIX_EPOCH;
    let unnamed = v1::CredentialEvent {
        event: Some(CredentialEventCase::Revoked(Box::default())),
    };
    vec![
        Unreadable {
            case: "no event type",
            message: stored(1, raw_message(&identity(), None, revocation.encode().unwrap())),
            reads_as: |error| matches!(error, InvalidationSourceError::Unrecorded(_)),
            retries: false,
        },
        Unreadable {
            case: "an unknown event type",
            message: stored(2, raw_message(&identity(), Some("acme.v1.Unknown"), Vec::new())),
            reads_as: |error| matches!(error, InvalidationSourceError::UnknownEventType { .. }),
            retries: true,
        },
        Unreadable {
            case: "a payload that does not decode",
            message: stored(3, raw_message(&identity(), Some(event_type), vec![0xff; 4])),
            reads_as: |error| matches!(error, InvalidationSourceError::Undecodable(_)),
            retries: false,
        },
        Unreadable {
            case: "a revocation naming no credential",
            message: stored(4, message(&identity(), &unnamed)),
            reads_as: |error| matches!(error, InvalidationSourceError::InvalidEvent(_)),
            retries: false,
        },
        Unreadable {
            case: "a revocation on another credential's subject",
            message: stored(
                5,
                raw_message(&other_identity(), Some(event_type), revocation.encode().unwrap()),
            ),
            reads_as: |error| matches!(error, InvalidationSourceError::EventFromAnotherSubject { .. }),
            retries: false,
        },
        Unreadable {
            case: "a revocation appended at the Unix epoch",
            message: at_epoch,
            reads_as: |error| matches!(error, InvalidationSourceError::NotAppendedAfterEpoch { .. }),
            retries: false,
        },
    ]
}

#[tokio::test]
async fn an_unreadable_event_is_terminated_unless_its_type_is_unknown_to_this_build() {
    for unreadable in unreadable_events() {
        let run = Run::new();
        let sequence = unreadable.message.sequence;
        let read = InvalidationOutcome::of(CREDENTIAL_STREAM, unreadable.message.clone(), &holding_owner());
        assert!(
            read.as_ref().is_err_and(unreadable.reads_as),
            "{}: {read:?}",
            unreadable.case
        );

        let outcome = run
            .publish([Ok(run.delivery(unreadable.message))], [], holding_owner())
            .await;

        ran_out(outcome);
        let settled = if unreadable.retries {
            retrying(sequence)
        } else {
            Step::Terminated(sequence)
        };
        assert_eq!(run.steps(), [settled], "{}", unreadable.case);
    }
}

#[test]
fn an_event_whose_owner_has_no_subject_token_is_never_retried() {
    let credential = identity().key();
    let error = InvalidationSourceError::NoSubject(CredentialEventSubjectResolverError::OutsideProject {
        owner_id: CredentialOwnerId::new(OWNER).unwrap(),
        stream_id: credential,
    });

    assert!(!retried(&error));
}

#[tokio::test]
async fn an_unreadable_event_is_logged_with_its_stream_sequence_and_why() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let run = Run::new();
    let undecodable = stored(
        3,
        raw_message(
            &identity(),
            Some(revoked(&identity()).event_type().unwrap()),
            vec![0xff; 4],
        ),
    );
    let unknown = stored(4, raw_message(&identity(), Some("acme.v1.Unknown"), Vec::new()));

    let outcome = run
        .publish(
            [Ok(run.delivery(undecodable)), Ok(run.delivery(unknown))],
            [],
            holding_owner(),
        )
        .await;

    ran_out(outcome);
    let terminated = logged_at(&events, 3);
    assert_eq!(terminated.len(), 1, "{terminated:?}");
    assert!(
        terminated[0]
            .field("error")
            .is_some_and(|error| error.starts_with("credential event payload does not decode")),
        "{terminated:?}"
    );
    let retried = logged_at(&events, 4);
    assert_eq!(retried.len(), 1, "{retried:?}");
    assert_eq!(
        retried[0].field("error"),
        Some("event type 'acme.v1.Unknown' is not a credential event this build knows")
    );
}

#[tokio::test]
async fn a_delivery_that_does_not_say_where_the_event_was_stored_is_retried() {
    let run = Run::new();

    let outcome = run.publish([Ok(run.unreadable(9))], [], holding_owner()).await;

    ran_out(outcome);
    assert_eq!(run.steps(), [retrying(9)]);
}

#[tokio::test]
async fn a_consumer_the_credential_stream_refuses_stops_the_publisher() {
    let run = Run::new();
    let events = TestEvents::answering(Err(MockError("fake-consumer-refused".to_owned())));

    let outcome = InvalidationPublisher::new(events, run.notices([]), holding_owner())
        .run(CancellationToken::new())
        .await;

    assert!(
        matches!(&outcome, Err(InvalidationPublisherError::Consumer(MockError(why))) if why == "fake-consumer-refused"),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn a_consumer_that_cannot_start_delivering_stops_the_publisher() {
    let run = Run::new();
    let events = TestEvents::answering(Ok(TestConsumer {
        messages: Mutex::new(Some(Err(MockError("fake-pull-refused".to_owned())))),
    }));

    let outcome = InvalidationPublisher::new(events, run.notices([]), holding_owner())
        .run(CancellationToken::new())
        .await;

    assert!(
        matches!(&outcome, Err(InvalidationPublisherError::Messages(MockError(why))) if why == "fake-pull-refused"),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn deliveries_that_end_before_shutdown_stop_the_publisher() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::ERROR);
    let run = Run::new();

    let outcome = run.publish([], [], holding_owner()).await;

    ran_out(outcome);
    assert!(
        events
            .events()
            .iter()
            .any(|event| event.field("durable") == Some(INVALIDATION_PUBLISHER_DURABLE)),
        "{:?}",
        events.events()
    );
}

#[tokio::test]
async fn events_that_cannot_be_published_do_not_hold_back_the_revocation_after_them() {
    let run = Run::new();
    let undecodable = stored(
        1,
        raw_message(
            &identity(),
            Some(revoked(&identity()).event_type().unwrap()),
            vec![0xff; 4],
        ),
    );
    let refused = stored(2, message(&identity(), &rotated(&identity())));
    let revocation = stored(3, message(&identity(), &revoked(&identity())));
    let refused_notice = notice_for(&refused, &holding_owner());
    let revocation_notice = notice_for(&revocation, &holding_owner());

    let outcome = run
        .publish(
            [
                Ok(run.delivery(undecodable)),
                Ok(run.unreadable(4)),
                Ok(run.delivery(refused)),
                Ok(run.delivery(revocation)),
            ],
            [Answer::Refused, Answer::Stored],
            holding_owner(),
        )
        .await;

    ran_out(outcome);
    assert_eq!(
        run.steps(),
        [
            Step::Terminated(1),
            retrying(4),
            published(&refused_notice),
            retrying(2),
            published(&revocation_notice),
            Step::Stored,
            Step::Acked(3),
        ]
    );
}

#[tokio::test]
async fn a_settlement_the_credential_stream_refuses_is_logged_and_reading_goes_on() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let run = Run::new();
    let unsettled = stored(1, message(&identity(), &activated(&identity())));
    let revocation = stored(2, message(&identity(), &revoked(&identity())));
    let notice = notice_for(&revocation, &holding_owner());

    let outcome = run
        .publish(
            [Ok(run.unsettleable(unsettled)), Ok(run.delivery(revocation))],
            [],
            holding_owner(),
        )
        .await;

    ran_out(outcome);
    assert_eq!(run.steps(), [published(&notice), Step::Stored, Step::Acked(2)]);
    let unsettled = logged_at(&events, 1);
    assert_eq!(unsettled.len(), 1, "{unsettled:?}");
    assert_eq!(unsettled[0].field("error"), Some("fake-settle-refused"));
}

#[tokio::test]
async fn a_retry_the_credential_stream_refuses_is_logged_and_reading_goes_on() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let run = Run::new();
    let unknown = stored(4, raw_message(&identity(), Some("acme.v1.Unknown"), Vec::new()));
    let revocation = stored(5, message(&identity(), &revoked(&identity())));
    let notice = notice_for(&revocation, &holding_owner());

    let outcome = run
        .publish(
            [Ok(run.unsettleable(unknown)), Ok(run.delivery(revocation))],
            [],
            holding_owner(),
        )
        .await;

    ran_out(outcome);
    assert_eq!(run.steps(), [published(&notice), Step::Stored, Step::Acked(5)]);
    let unsettled: Vec<_> = logged_at(&events, 4)
        .into_iter()
        .filter(|event| event.field("settlement").is_some())
        .collect();
    assert_eq!(unsettled.len(), 1, "{unsettled:?}");
    assert!(
        unsettled[0]
            .field("settlement")
            .is_some_and(|kind| kind.starts_with("Nak")),
        "{unsettled:?}"
    );
    assert_eq!(unsettled[0].field("error"), Some("fake-settle-refused"));
}

#[tokio::test]
async fn a_delivery_that_fails_is_logged_and_reading_goes_on() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let run = Run::new();
    let revocation = stored(2, message(&identity(), &revoked(&identity())));
    let notice = notice_for(&revocation, &holding_owner());

    let outcome = run
        .publish(
            [
                Err(MockError("fake-missed-heartbeat".to_owned())),
                Ok(run.delivery(revocation)),
            ],
            [],
            holding_owner(),
        )
        .await;

    ran_out(outcome);
    assert_eq!(run.steps(), [published(&notice), Step::Stored, Step::Acked(2)]);
    assert!(
        events
            .events()
            .iter()
            .any(|event| event.field("error") == Some("fake-missed-heartbeat")),
        "{:?}",
        events.events()
    );
}

#[tokio::test]
async fn an_event_is_not_acknowledged_until_the_invalidation_stream_stores_its_notice() {
    let run = Run::new();
    let revocation = stored(5, message(&identity(), &revoked(&identity())));
    let notice = notice_for(&revocation, &holding_owner());
    let (release, held) = oneshot::channel();
    let (deliveries, delivered) = mpsc::unbounded();
    deliveries.unbounded_send(Ok(run.delivery(revocation))).unwrap();
    let events = TestEvents::delivering(delivered.boxed());
    let publisher = InvalidationPublisher::new(events, run.notices([Answer::Held(held)]), holding_owner());
    let running = tokio::spawn(publisher.run(CancellationToken::new()));

    while run.steps().is_empty() {
        tokio::task::yield_now().await;
    }
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        run.steps(),
        [published(&notice)],
        "acknowledged before the notice was stored"
    );

    release.send(()).unwrap();
    drop(deliveries);

    ran_out(running.await.unwrap());
    assert_eq!(run.steps(), [published(&notice), Step::Stored, Step::Acked(5)]);
}

#[tokio::test]
async fn a_revocation_and_a_rotation_each_publish_their_notice_and_are_acknowledged_once_it_is_stored() {
    let run = Run::new();
    let revocation = stored(1, message(&identity(), &revoked(&identity())));
    let rotation = stored(2, message(&identity(), &rotated(&identity())));
    let revocation_notice = notice_for(&revocation, &holding_owner());
    let rotation_notice = notice_for(&rotation, &holding_owner());

    let outcome = run
        .publish(
            [Ok(run.delivery(revocation)), Ok(run.delivery(rotation))],
            [],
            holding_owner(),
        )
        .await;

    ran_out(outcome);
    assert_eq!(
        run.steps(),
        [
            published(&revocation_notice),
            Step::Stored,
            Step::Acked(1),
            published(&rotation_notice),
            Step::Stored,
            Step::Acked(2),
        ]
    );
}

#[tokio::test]
async fn an_event_that_leaves_the_release_as_it_was_is_acknowledged_without_a_notice() {
    let run = Run::new();
    let activation = stored(3, message(&identity(), &activated(&identity())));

    let outcome = run.publish([Ok(run.delivery(activation))], [], holding_owner()).await;

    ran_out(outcome);
    assert_eq!(run.steps(), [Step::Acked(3)]);
}

#[tokio::test]
async fn an_event_whose_owner_no_tenant_holds_is_acknowledged_and_logged_by_credential() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let run = Run::new();
    let revocation = stored(8, message(&identity(), &revoked(&identity())));

    let outcome = run
        .publish([Ok(run.delivery(revocation))], [], StaticTenantOwners::new())
        .await;

    ran_out(outcome);
    assert_eq!(run.steps(), [Step::Acked(8)]);
    let logged = logged_at(&events, 8);
    assert_eq!(logged.len(), 1, "{logged:?}");
    let credential = identity().key();
    assert_eq!(logged[0].field("owner_id"), Some(OWNER));
    assert_eq!(
        logged[0].field("credential_id"),
        Some(credential.credential_id().to_string().as_str())
    );
}

#[tokio::test]
async fn the_publisher_reads_new_events_through_its_shared_durable_and_acknowledges_each_explicitly() {
    let run = Run::new();
    let events = TestEvents::delivering(stream::empty().boxed());

    let _ = InvalidationPublisher::new(events.clone(), run.notices([]), holding_owner())
        .run(CancellationToken::new())
        .await;

    let configs = events.configs();
    assert_eq!(configs.len(), 1);
    let config = &configs[0];
    assert_eq!(config.durable_name.as_deref(), Some(INVALIDATION_PUBLISHER_DURABLE));
    assert_eq!(config.filter_subject, "credentials.events.v1.>");
    assert_eq!(config.ack_policy, AckPolicy::Explicit);
    assert_eq!(config.deliver_policy, DeliverPolicy::New);
    assert_eq!(config.ack_wait, INVALIDATION_PUBLISHER_ACK_WAIT);
}

#[tokio::test]
async fn an_event_appended_once_the_publisher_listens_is_published() {
    let run = Run::new();
    let (deliveries, delivered) = mpsc::unbounded::<Result<TestDelivery, MockError>>();
    let events = TestEvents::delivering(delivered.boxed());
    let listening = InvalidationPublisher::new(events.clone(), run.notices([]), holding_owner())
        .listen(&CancellationToken::new())
        .await
        .unwrap()
        .expect("listening before shutdown");
    assert_eq!(
        events.configs().len(),
        1,
        "listening returned before its durable existed"
    );
    let revocation = stored(1, message(&identity(), &revoked(&identity())));
    let notice = notice_for(&revocation, &holding_owner());

    deliveries.unbounded_send(Ok(run.delivery(revocation))).unwrap();
    drop(deliveries);

    ran_out(listening.run(CancellationToken::new()).await);
    assert_eq!(run.steps(), [published(&notice), Step::Stored, Step::Acked(1)]);
}

#[tokio::test]
async fn a_consumer_the_credential_stream_refuses_is_reported_by_listening() {
    let run = Run::new();
    let events = TestEvents::answering(Err(MockError("fake-consumer-refused".to_owned())));

    let listening = InvalidationPublisher::new(events, run.notices([]), holding_owner())
        .listen(&CancellationToken::new())
        .await;

    assert!(
        matches!(&listening, Err(InvalidationPublisherError::Consumer(MockError(why))) if why == "fake-consumer-refused"),
        "listening did not report the refused consumer"
    );
}

#[tokio::test]
async fn shutdown_before_the_publisher_listens_creates_no_durable() {
    let run = Run::new();
    let events = TestEvents::delivering(stream::empty().boxed());
    let shutdown = CancellationToken::new();
    shutdown.cancel();

    let listening = InvalidationPublisher::new(events.clone(), run.notices([]), holding_owner())
        .listen(&shutdown)
        .await;

    assert!(matches!(listening, Ok(None)), "listening went on after shutdown");
    assert!(events.configs().is_empty());
}

#[tokio::test]
async fn shutdown_stops_the_publisher_without_an_error() {
    let run = Run::new();
    let (_deliveries, delivered) = mpsc::unbounded::<Result<TestDelivery, MockError>>();
    let events = TestEvents::delivering(delivered.boxed());
    let shutdown = CancellationToken::new();
    let running =
        tokio::spawn(InvalidationPublisher::new(events, run.notices([]), holding_owner()).run(shutdown.clone()));

    shutdown.cancel();

    assert!(running.await.unwrap().is_ok());
}

mod jetstream_tests;
