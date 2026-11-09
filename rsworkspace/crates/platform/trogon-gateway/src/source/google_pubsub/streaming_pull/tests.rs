use super::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{ClaimBucket, ClaimBucketBinding};
use trogon_nats::jetstream::{MaxPayload, MockJetStreamPublisher, MockObjectStore, StreamMaxAge};
use trogon_std::NonZeroDuration;

use crate::source::google_pubsub::credentials::fixtures::fake_credentials;
use crate::source::google_pubsub::max_outstanding_messages::MaxOutstandingMessages;

const SUBSCRIPTION: &str = "projects/p/subscriptions/sub";

fn wrap_publisher(publisher: MockJetStreamPublisher) -> ClaimCheckPublisher<MockJetStreamPublisher, MockObjectStore> {
    ClaimCheckPublisher::new(
        publisher,
        ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
        MaxPayload::from_server_limit(usize::MAX),
    )
}

fn test_config() -> GooglePubSubConfig {
    GooglePubSubConfig {
        subject_prefix: NatsToken::new("google-pubsub-primary").unwrap(),
        stream_name: NatsToken::new("GOOGLE_PUBSUB_PRIMARY").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        transport: GooglePubSubTransportConfig::StreamingPull(StreamingPullConfig {
            subscription: PubSubSubscriptionName::new(SUBSCRIPTION).unwrap(),
            max_outstanding_messages: MaxOutstandingMessages::new(25).ok(),
            credentials: fake_credentials(),
        }),
    }
}

fn pulled(id: &str, data: &'static [u8]) -> PulledMessage {
    PulledMessage {
        data: Bytes::from_static(data),
        message_id: id.to_string(),
        publish_time: Some("2026-01-02T03:04:05Z".to_string()),
        ordering_key: String::new(),
        attributes: BTreeMap::new(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Settlement {
    Acked(String),
    Nacked(String),
}

type Settlements = Arc<Mutex<Vec<Settlement>>>;

struct FakeHandle {
    id: String,
    settlements: Settlements,
}

impl MessageHandle for FakeHandle {
    fn ack(self) {
        self.settlements.lock().unwrap().push(Settlement::Acked(self.id));
    }

    fn nack(self) {
        self.settlements.lock().unwrap().push(Settlement::Nacked(self.id));
    }
}

enum Step {
    Message(PulledMessage),
    Fail(&'static str),
}

enum Tail {
    Close,
    Hang,
}

struct FakeStream {
    steps: VecDeque<Step>,
    tail: Tail,
    settlements: Settlements,
    shutdowns: Arc<AtomicUsize>,
}

impl PullStream for FakeStream {
    type Handle = FakeHandle;

    async fn next(&mut self) -> Option<Result<(PulledMessage, FakeHandle), PullError>> {
        match self.steps.pop_front() {
            Some(Step::Message(message)) => {
                let handle = FakeHandle {
                    id: message.message_id.clone(),
                    settlements: self.settlements.clone(),
                };
                Some(Ok((message, handle)))
            }
            Some(Step::Fail(reason)) => Some(Err(PullError::Stream(reason.into()))),
            None => match self.tail {
                Tail::Close => None,
                Tail::Hang => std::future::pending().await,
            },
        }
    }

    async fn shutdown(&mut self) {
        self.shutdowns.fetch_add(1, Ordering::SeqCst);
    }
}

enum Open {
    Fail(&'static str),
    Stream(Vec<Step>, Tail),
}

struct FakeSource {
    opens: VecDeque<Open>,
    settlements: Settlements,
    shutdowns: Arc<AtomicUsize>,
    open_times: Arc<Mutex<Vec<Instant>>>,
    max_outstanding: Arc<Mutex<Vec<Option<u32>>>>,
}

impl SubscriptionSource for FakeSource {
    type Stream = FakeStream;

    async fn open<'a>(&'a mut self, config: &'a StreamingPullConfig) -> Result<FakeStream, PullError> {
        assert_eq!(config.subscription.as_str(), SUBSCRIPTION);
        self.open_times.lock().unwrap().push(Instant::now());
        self.max_outstanding
            .lock()
            .unwrap()
            .push(config.max_outstanding_messages.map(MaxOutstandingMessages::get));
        match self.opens.pop_front() {
            Some(Open::Fail(reason)) => Err(PullError::Open(reason.into())),
            Some(Open::Stream(steps, tail)) => Ok(FakeStream {
                steps: steps.into(),
                tail,
                settlements: self.settlements.clone(),
                shutdowns: self.shutdowns.clone(),
            }),
            None => std::future::pending().await,
        }
    }
}

struct Harness {
    publisher: MockJetStreamPublisher,
    settlements: Settlements,
    shutdowns: Arc<AtomicUsize>,
    open_times: Arc<Mutex<Vec<Instant>>>,
    max_outstanding: Arc<Mutex<Vec<Option<u32>>>>,
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl Harness {
    fn start(opens: Vec<Open>) -> Self {
        Self::start_with(MockJetStreamPublisher::new(), opens)
    }

    fn start_with(publisher: MockJetStreamPublisher, opens: Vec<Open>) -> Self {
        let settlements = Settlements::default();
        let shutdowns = Arc::new(AtomicUsize::new(0));
        let open_times = Arc::new(Mutex::new(Vec::new()));
        let max_outstanding = Arc::new(Mutex::new(Vec::new()));
        let source = FakeSource {
            opens: opens.into(),
            settlements: settlements.clone(),
            shutdowns: shutdowns.clone(),
            open_times: open_times.clone(),
            max_outstanding: max_outstanding.clone(),
        };
        let (shutdown, shutdown_rx) = oneshot::channel::<()>();
        let task = tokio::spawn(run(
            wrap_publisher(publisher.clone()),
            test_config(),
            source,
            async move {
                shutdown_rx.await.ok();
            },
        ));
        Self {
            publisher,
            settlements,
            shutdowns,
            open_times,
            max_outstanding,
            shutdown,
            task,
        }
    }

    async fn settle(&self, seconds: u64) {
        tokio::time::sleep(Duration::from_secs(seconds)).await;
    }

    async fn stop(self) -> Self {
        self.shutdown.send(()).unwrap();
        self.task.await.unwrap();
        Self {
            shutdown: oneshot::channel().0,
            task: tokio::spawn(async {}),
            ..self
        }
    }

    fn settlements(&self) -> Vec<Settlement> {
        self.settlements.lock().unwrap().clone()
    }

    fn open_gaps(&self) -> Vec<Duration> {
        let times = self.open_times.lock().unwrap();
        times.windows(2).map(|pair| pair[1] - pair[0]).collect()
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let name = HeaderName::try_from(name).ok()?;
    headers.get(name).map(|value| value.as_str())
}

#[test]
fn resubscribe_delay_doubles_until_cap() {
    assert_eq!(next_resubscribe_delay(Duration::from_secs(1)), Duration::from_secs(2));
    assert_eq!(next_resubscribe_delay(Duration::from_secs(20)), RESUBSCRIBE_MAX_DELAY);
    assert_eq!(next_resubscribe_delay(RESUBSCRIBE_MAX_DELAY), RESUBSCRIBE_MAX_DELAY);
}

#[tokio::test(start_paused = true)]
async fn message_is_published_verbatim_then_acked() {
    let mut message = pulled("m-1", b"\x00raw bytes, not json\xff");
    message.ordering_key = "order-1".to_string();
    message.attributes.insert("tenant".to_string(), "acme".to_string());
    let harness = Harness::start(vec![Open::Stream(vec![Step::Message(message)], Tail::Hang)]);

    harness.settle(5).await;
    let harness = harness.stop().await;

    let published = harness.publisher.published_messages();
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].subject, "google-pubsub-primary.message");
    assert_eq!(published[0].payload, Bytes::from_static(b"\x00raw bytes, not json\xff"));
    assert_eq!(header(&published[0].headers, "Nats-Msg-Id"), Some("m-1"));
    assert_eq!(header(&published[0].headers, NATS_HEADER_MESSAGE_ID), Some("m-1"));
    assert_eq!(
        header(&published[0].headers, NATS_HEADER_PUBLISH_TIME),
        Some("2026-01-02T03:04:05Z")
    );
    assert_eq!(header(&published[0].headers, NATS_HEADER_ORDERING_KEY), Some("order-1"));
    assert_eq!(
        header(&published[0].headers, NATS_HEADER_SUBSCRIPTION),
        Some(SUBSCRIPTION)
    );
    assert_eq!(header(&published[0].headers, "X-PubSub-Attribute-tenant"), Some("acme"));
    assert_eq!(harness.settlements(), vec![Settlement::Acked("m-1".to_string())]);
    assert_eq!(harness.shutdowns.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn failed_publish_is_nacked_and_not_acked() {
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let harness = Harness::start_with(
        publisher,
        vec![Open::Stream(
            vec![
                Step::Message(pulled("m-1", b"first")),
                Step::Message(pulled("m-2", b"second")),
            ],
            Tail::Hang,
        )],
    );

    harness.settle(5).await;
    let harness = harness.stop().await;

    assert_eq!(
        harness.settlements(),
        vec![
            Settlement::Nacked("m-1".to_string()),
            Settlement::Acked("m-2".to_string())
        ]
    );
    let published = harness.publisher.published_messages();
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].payload, Bytes::from_static(b"second"));
}

#[tokio::test(start_paused = true)]
async fn nack_pauses_before_pulling_again() {
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let harness = Harness::start_with(
        publisher,
        vec![Open::Stream(
            vec![
                Step::Message(pulled("m-1", b"first")),
                Step::Message(pulled("m-2", b"second")),
            ],
            Tail::Hang,
        )],
    );

    tokio::time::sleep(REDELIVERY_PAUSE / 2).await;
    assert_eq!(harness.settlements(), vec![Settlement::Nacked("m-1".to_string())]);

    tokio::time::sleep(REDELIVERY_PAUSE).await;
    assert_eq!(harness.settlements().len(), 2);
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn messages_are_published_in_pull_order() {
    let harness = Harness::start(vec![Open::Stream(
        vec![
            Step::Message(pulled("m-1", b"one")),
            Step::Message(pulled("m-2", b"two")),
            Step::Message(pulled("m-3", b"three")),
        ],
        Tail::Hang,
    )]);

    harness.settle(5).await;
    let harness = harness.stop().await;

    assert_eq!(
        harness.publisher.published_payloads(),
        vec![
            Bytes::from_static(b"one"),
            Bytes::from_static(b"two"),
            Bytes::from_static(b"three")
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn stream_errors_resubscribe_with_growing_backoff() {
    let harness = Harness::start(vec![
        Open::Stream(vec![Step::Fail("boom")], Tail::Hang),
        Open::Fail("denied"),
        Open::Stream(vec![Step::Message(pulled("m-1", b"after"))], Tail::Hang),
    ]);

    harness.settle(60).await;
    let harness = harness.stop().await;

    assert_eq!(
        harness.open_gaps(),
        vec![Duration::from_secs(1), Duration::from_secs(2)]
    );
    assert_eq!(harness.settlements(), vec![Settlement::Acked("m-1".to_string())]);
    assert_eq!(harness.shutdowns.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn backoff_resets_after_a_session_delivers_a_message() {
    let harness = Harness::start(vec![
        Open::Fail("one"),
        Open::Fail("two"),
        Open::Stream(vec![Step::Message(pulled("m-1", b"x")), Step::Fail("lost")], Tail::Hang),
        Open::Fail("three"),
    ]);

    harness.settle(60).await;
    let harness = harness.stop().await;

    assert_eq!(
        harness.open_gaps(),
        vec![
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(1),
            Duration::from_secs(2),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn ended_stream_is_resubscribed() {
    let harness = Harness::start(vec![
        Open::Stream(vec![Step::Message(pulled("m-1", b"a"))], Tail::Close),
        Open::Stream(vec![Step::Message(pulled("m-2", b"b"))], Tail::Hang),
    ]);

    harness.settle(10).await;
    let harness = harness.stop().await;

    assert_eq!(
        harness.settlements(),
        vec![
            Settlement::Acked("m-1".to_string()),
            Settlement::Acked("m-2".to_string())
        ]
    );
    assert_eq!(harness.open_times.lock().unwrap().len(), 2);
    assert_eq!(harness.shutdowns.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn max_outstanding_messages_is_passed_to_the_source() {
    let harness = Harness::start(vec![Open::Stream(vec![], Tail::Hang)]);

    harness.settle(1).await;
    let harness = harness.stop().await;

    assert_eq!(*harness.max_outstanding.lock().unwrap(), vec![Some(25)]);
}

#[tokio::test(start_paused = true)]
async fn shutdown_while_waiting_for_messages_shuts_the_stream_down() {
    let harness = Harness::start(vec![Open::Stream(vec![], Tail::Hang)]);

    harness.settle(1).await;
    let harness = harness.stop().await;

    assert_eq!(harness.shutdowns.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn shutdown_while_opening_returns_without_a_stream() {
    let harness = Harness::start(vec![]);

    harness.settle(1).await;
    let harness = harness.stop().await;

    assert_eq!(harness.shutdowns.load(Ordering::SeqCst), 0);
    assert!(harness.settlements().is_empty());
}

#[tokio::test(start_paused = true)]
async fn shutdown_during_resubscribe_backoff_returns_promptly() {
    let harness = Harness::start(vec![Open::Fail("down")]);

    tokio::time::sleep(RESUBSCRIBE_INITIAL_DELAY / 2).await;
    let harness = harness.stop().await;

    assert_eq!(harness.open_times.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn shutdown_during_nack_pause_shuts_the_stream_down() {
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let harness = Harness::start_with(
        publisher,
        vec![Open::Stream(
            vec![
                Step::Message(pulled("m-1", b"first")),
                Step::Message(pulled("m-2", b"second")),
            ],
            Tail::Hang,
        )],
    );

    tokio::time::sleep(REDELIVERY_PAUSE / 2).await;
    let harness = harness.stop().await;

    assert_eq!(harness.settlements(), vec![Settlement::Nacked("m-1".to_string())]);
    assert_eq!(harness.shutdowns.load(Ordering::SeqCst), 1);
}

fn subscription() -> PubSubSubscriptionName {
    PubSubSubscriptionName::new(SUBSCRIPTION).unwrap()
}

#[test]
fn headers_omit_empty_ordering_key_and_missing_publish_time() {
    let mut message = pulled("m-1", b"x");
    message.publish_time = None;

    let headers = message_headers(&message, &subscription());

    assert_eq!(header(&headers, NATS_HEADER_ORDERING_KEY), None);
    assert_eq!(header(&headers, NATS_HEADER_PUBLISH_TIME), None);
    assert_eq!(header(&headers, NATS_HEADER_SUBSCRIPTION), Some(SUBSCRIPTION));
}

#[test]
fn headers_omit_message_id_when_it_is_empty() {
    let message = pulled("", b"x");

    let headers = message_headers(&message, &subscription());

    assert_eq!(header(&headers, "Nats-Msg-Id"), None);
    assert_eq!(header(&headers, NATS_HEADER_MESSAGE_ID), None);
}

#[test]
fn attributes_cannot_overwrite_reserved_headers() {
    let mut message = pulled("m-1", b"x");
    message
        .attributes
        .insert("Nats-Msg-Id".to_string(), "forged".to_string());
    message
        .attributes
        .insert("Nats-Claim-Check".to_string(), "forged".to_string());

    let headers = message_headers(&message, &subscription());

    assert_eq!(header(&headers, "Nats-Msg-Id"), Some("m-1"));
    assert_eq!(header(&headers, "Nats-Claim-Check"), None);
    assert_eq!(header(&headers, "X-PubSub-Attribute-Nats-Msg-Id"), Some("forged"));
}

#[test]
fn invalid_attributes_are_skipped_without_panicking() {
    let mut message = pulled("m-1", b"x");
    message.attributes.insert("bad key".to_string(), "v".to_string());
    message.attributes.insert("bad:key".to_string(), "v".to_string());
    message.attributes.insert("caf\u{e9}".to_string(), "v".to_string());
    message.attributes.insert(String::new(), "v".to_string());
    message
        .attributes
        .insert("multi-line".to_string(), "line1\r\nline2".to_string());
    message.attributes.insert("good".to_string(), "kept".to_string());

    let headers = message_headers(&message, &subscription());

    assert_eq!(header(&headers, "X-PubSub-Attribute-good"), Some("kept"));
    assert_eq!(header(&headers, "X-PubSub-Attribute-multi-line"), None);
    assert_eq!(header(&headers, "X-PubSub-Attribute-bad key"), None);
    assert_eq!(header(&headers, "X-PubSub-Attribute-bad:key"), None);
}

#[test]
fn ordering_key_with_line_break_is_skipped_without_panicking() {
    let mut message = pulled("m-1", b"x");
    message.ordering_key = "a\nb".to_string();

    let headers = message_headers(&message, &subscription());

    assert_eq!(header(&headers, NATS_HEADER_ORDERING_KEY), None);
    assert_eq!(header(&headers, "Nats-Msg-Id"), Some("m-1"));
}

#[test]
fn pull_errors_describe_their_phase() {
    assert_eq!(
        PullError::Open("denied".into()).to_string(),
        "failed to open Pub/Sub subscription stream: denied"
    );
    assert_eq!(
        PullError::Stream("reset".into()).to_string(),
        "Pub/Sub stream failed: reset"
    );
}
