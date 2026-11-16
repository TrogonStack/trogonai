use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use acp_nats::{
    AcpPrefix, AcpSessionId, AgentHandler, Bridge, ClientHandler, Config, FlushClient, NatsClientProxy, NatsConfig,
    PublishClient, RequestClient, SubscribeClient,
};
use agent_client_protocol::ErrorCode;
use agent_client_protocol::v1::schema::{
    CancelNotification, ContentBlock, ContentChunk, PromptRequest, PromptResponse, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SessionNotification, SessionUpdate, StopReason,
    ToolCallUpdate, ToolCallUpdateFields,
};
use async_nats::subject::ToSubject;
use async_nats::{HeaderMap, Message};
use bytes::Bytes;
use futures::channel::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use trogon_nats::AdvancedMockNatsClient;
use trogon_nats::jetstream::{
    JetStreamGetStream, JetStreamPublisher, MockJetStreamConsumer, MockJetStreamConsumerFactory,
    MockJetStreamPublisher, MockJetStreamStream, MockJsMessage,
};
use trogon_nats::mocks::MockError;
use trogon_std::time::SystemClock;

type TestResult = Result<(), Box<dyn std::error::Error>>;
type ResponseSender = UnboundedSender<Result<MockJsMessage, MockError>>;

const SESSION: &str = "session-1";
const PREFIX: &str = "cancel_contract";
const CANCEL: &str = "cancel_contract.v1.session.session-1.agent.cancel";
const CANCELLED: &str = "cancel_contract.v1.session.session-1.agent.cancelled";
const PROMPT: &str = "cancel_contract.v1.session.session-1.agent.prompt";
const PERMISSION: &str = "cancel_contract.v1.session.session-1.client.session.request_permission";
const UPDATE: &str = "cancel_contract.v1.session.session-1.client.session.update";
const PROMPT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug)]
enum CancelDelivery {
    Accepted,
    Refused,
}

struct PermissionRequest {
    headers: HeaderMap,
    response: oneshot::Sender<Message>,
}

#[derive(Clone)]
struct ControlledNats {
    client: AdvancedMockNatsClient,
    legacy_events: UnboundedSender<Message>,
    cancel_delivery: CancelDelivery,
    failed_cancel_attempts: Arc<AtomicUsize>,
    permissions: Arc<Mutex<VecDeque<PermissionRequest>>>,
}

impl SubscribeClient for ControlledNats {
    type SubscribeError = <AdvancedMockNatsClient as SubscribeClient>::SubscribeError;
    type Subscription = <AdvancedMockNatsClient as SubscribeClient>::Subscription;

    async fn subscribe<S: ToSubject + Send>(&self, subject: S) -> Result<Self::Subscription, Self::SubscribeError> {
        self.client.subscribe(subject).await
    }
}

impl RequestClient for ControlledNats {
    type RequestError = MockError;

    async fn request_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> Result<Message, Self::RequestError> {
        let subject = subject.to_subject();
        if subject.as_str() == PERMISSION {
            let (response, received) = oneshot::channel();
            self.permissions
                .lock()
                .map_err(|_| MockError("permission requests lock poisoned".to_owned()))?
                .push_back(PermissionRequest { headers, response });
            received
                .await
                .map_err(|_| MockError("permission response channel closed".to_owned()))
        } else {
            self.client.request_with_headers(subject, headers, payload).await
        }
    }
}

impl PublishClient for ControlledNats {
    type PublishError = MockError;

    async fn publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> Result<(), Self::PublishError> {
        let subject = subject.to_subject().to_string();
        if subject == CANCEL && matches!(self.cancel_delivery, CancelDelivery::Refused) {
            self.failed_cancel_attempts.fetch_add(1, Ordering::SeqCst);
            return Err(MockError("backend cancellation publication refused".to_owned()));
        }
        self.client
            .publish_with_headers(subject.clone(), headers.clone(), payload.clone())
            .await?;
        if subject == CANCELLED {
            self.legacy_events
                .unbounded_send(message(&subject, headers, payload))
                .map_err(|_| MockError("legacy event channel closed".to_owned()))?;
        }
        Ok(())
    }
}

impl FlushClient for ControlledNats {
    type FlushError = MockError;

    async fn flush(&self) -> Result<(), Self::FlushError> {
        self.client.flush().await
    }
}

#[derive(Clone)]
struct ControlledJetStream {
    publisher: MockJetStreamPublisher,
    consumers: MockJetStreamConsumerFactory,
}

impl JetStreamPublisher for ControlledJetStream {
    type PublishError = MockError;
    type AckFuture = std::future::Ready<Result<async_nats::jetstream::publish::PublishAck, MockError>>;

    async fn publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> Result<Self::AckFuture, Self::PublishError> {
        self.publisher.publish_with_headers(subject, headers, payload).await
    }
}

impl JetStreamGetStream for ControlledJetStream {
    type Error = async_nats::jetstream::context::GetStreamError;
    type Stream = MockJetStreamStream;

    async fn get_stream<T: AsRef<str> + Send>(&self, name: T) -> Result<Self::Stream, Self::Error> {
        self.consumers.get_stream(name).await
    }
}

struct Fixture {
    nats: ControlledNats,
    js: ControlledJetStream,
    runner_responses: ResponseSender,
    bridge: Bridge<ControlledNats, SystemClock, ControlledJetStream>,
}

impl Fixture {
    fn new(cancel_delivery: CancelDelivery) -> Result<Self, Box<dyn std::error::Error>> {
        let client = AdvancedMockNatsClient::new();
        let nats = ControlledNats {
            legacy_events: client.inject_messages(),
            client,
            cancel_delivery,
            failed_cancel_attempts: Arc::new(AtomicUsize::new(0)),
            permissions: Arc::new(Mutex::new(VecDeque::new())),
        };
        let js = ControlledJetStream {
            publisher: MockJetStreamPublisher::new(),
            consumers: MockJetStreamConsumerFactory::new(),
        };
        let (consumer, runner_responses) = MockJetStreamConsumer::new();
        js.consumers.add_consumer(consumer);
        let config = Config::new(AcpPrefix::new(PREFIX)?, NatsConfig::from_url("localhost:4222"))
            .with_prompt_timeout(PROMPT_TIMEOUT);
        let bridge = Bridge::new(
            nats.clone(),
            js.clone(),
            SystemClock,
            &opentelemetry::global::meter("prompt-cancellation"),
            config,
        );
        Ok(Self {
            nats,
            js,
            runner_responses,
            bridge,
        })
    }

    fn runner_response(
        &self,
        request_index: usize,
        reason: StopReason,
    ) -> Result<MockJsMessage, Box<dyn std::error::Error>> {
        let requests = self.js.publisher.published_messages();
        let request = requests.get(request_index).ok_or("prompt has not been published")?;
        assert_eq!(request.subject, PROMPT);
        let id = acp_nats::wire::response_id_from_request_headers(&request.headers);
        let encoded = acp_nats::wire::encode_success(id, &PromptResponse::new(reason))?;
        Ok(MockJsMessage::new(message(
            "cancel_contract.v1.session.session-1.agent.response",
            encoded.headers,
            encoded.body,
        )))
    }

    fn settle_runner(&self, request_index: usize, reason: StopReason) -> TestResult {
        self.runner_responses
            .unbounded_send(Ok(self.runner_response(request_index, reason)?))?;
        Ok(())
    }

    fn settle_permission(&self) -> TestResult {
        let request = self
            .nats
            .permissions
            .lock()
            .map_err(|_| "permission requests lock poisoned")?
            .pop_front()
            .ok_or("permission callback has not been requested")?;
        let response = RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled);
        let encoded = acp_nats::wire::encode_success(
            acp_nats::wire::response_id_from_request_headers(&request.headers),
            &response,
        )?;
        request
            .response
            .send(message(PERMISSION, encoded.headers, encoded.body))
            .map_err(|_| "permission callback was dropped before client settlement")?;
        Ok(())
    }

    fn client(&self) -> Result<NatsClientProxy<ControlledNats>, Box<dyn std::error::Error>> {
        Ok(NatsClientProxy::new(
            self.nats.clone(),
            AcpSessionId::new(SESSION)?,
            AcpPrefix::new(PREFIX)?,
            PROMPT_TIMEOUT,
        ))
    }
}

fn message(subject: &str, headers: HeaderMap, payload: Bytes) -> Message {
    Message {
        subject: subject.to_owned().into(),
        reply: None,
        headers: Some(headers),
        length: payload.len(),
        payload,
        status: None,
        description: None,
    }
}

fn prompt() -> PromptRequest {
    PromptRequest::new(SESSION, vec![ContentBlock::from("Please continue the task.")])
}

async fn assert_cancel_waits_for_runner(delivery: CancelDelivery, terminal_reason: StopReason) -> TestResult {
    let fixture = Fixture::new(delivery)?;
    let pending = fixture.bridge.prompt(prompt());
    tokio::pin!(pending);
    assert!(futures::poll!(pending.as_mut()).is_pending());
    assert_eq!(fixture.js.publisher.published_subjects(), [PROMPT]);

    fixture.bridge.cancel(CancelNotification::new(SESSION)).await?;
    let published = fixture.nats.client.published_messages();
    match delivery {
        CancelDelivery::Accepted => {
            assert!(published.iter().any(|subject| subject == CANCEL));
            assert_eq!(fixture.nats.failed_cancel_attempts.load(Ordering::SeqCst), 0);
        }
        CancelDelivery::Refused => {
            assert!(!published.iter().any(|subject| subject == CANCEL));
            assert_eq!(fixture.nats.failed_cancel_attempts.load(Ordering::SeqCst), 1);
        }
    }

    let before_runner_settlement = futures::poll!(pending.as_mut());
    assert!(
        before_runner_settlement.is_pending(),
        "prompt completed before the runner settled: {before_runner_settlement:?}"
    );
    fixture.settle_runner(0, terminal_reason)?;
    assert_eq!(pending.await?.stop_reason, terminal_reason);
    Ok(())
}

#[tokio::test]
async fn delivered_cancel_waits_for_the_runners_terminal_prompt_response() -> TestResult {
    assert_cancel_waits_for_runner(CancelDelivery::Accepted, StopReason::Cancelled).await
}

#[tokio::test]
async fn failed_cancel_publication_does_not_complete_the_prompt_as_cancelled() -> TestResult {
    assert_cancel_waits_for_runner(CancelDelivery::Refused, StopReason::EndTurn).await
}

#[tokio::test]
async fn a_correlated_runner_response_completes_a_pending_prompt() -> TestResult {
    let fixture = Fixture::new(CancelDelivery::Accepted)?;
    let pending = fixture.bridge.prompt(prompt());
    tokio::pin!(pending);
    assert!(futures::poll!(pending.as_mut()).is_pending());
    fixture.settle_runner(0, StopReason::EndTurn)?;
    assert_eq!(pending.await?.stop_reason, StopReason::EndTurn);
    Ok(())
}

#[tokio::test]
async fn cancellation_intent_does_not_publish_a_settlement() -> TestResult {
    let fixture = Fixture::new(CancelDelivery::Accepted)?;
    fixture.bridge.cancel(CancelNotification::new(SESSION)).await?;
    assert_eq!(fixture.nats.client.published_messages(), [CANCEL]);
    Ok(())
}

#[tokio::test]
async fn an_uncorrelated_cancelled_broadcast_does_not_settle_the_prompt() -> TestResult {
    let fixture = Fixture::new(CancelDelivery::Accepted)?;
    let pending = fixture.bridge.prompt(prompt());
    tokio::pin!(pending);
    assert!(futures::poll!(pending.as_mut()).is_pending());
    fixture
        .nats
        .legacy_events
        .unbounded_send(message(CANCELLED, HeaderMap::new(), Bytes::new()))?;
    assert!(futures::poll!(pending.as_mut()).is_pending());
    fixture.settle_runner(0, StopReason::EndTurn)?;
    assert_eq!(pending.await?.stop_reason, StopReason::EndTurn);
    Ok(())
}

#[tokio::test]
async fn repeated_cancellation_does_not_settle_the_next_prompt_on_the_session() -> TestResult {
    let fixture = Fixture::new(CancelDelivery::Accepted)?;
    let first = fixture.bridge.prompt(prompt());
    tokio::pin!(first);
    assert!(futures::poll!(first.as_mut()).is_pending());
    for _ in 0..2 {
        fixture.bridge.cancel(CancelNotification::new(SESSION)).await?;
        assert!(futures::poll!(first.as_mut()).is_pending());
    }
    let stale = fixture.runner_response(0, StopReason::Cancelled)?;
    fixture.runner_responses.unbounded_send(Ok(stale.clone()))?;
    assert_eq!(first.await?.stop_reason, StopReason::Cancelled);

    let (consumer, replies) = MockJetStreamConsumer::new();
    fixture.js.consumers.add_consumer(consumer);
    let next = fixture.bridge.prompt(prompt());
    tokio::pin!(next);
    assert!(futures::poll!(next.as_mut()).is_pending());
    replies.unbounded_send(Ok(stale))?;
    assert!(futures::poll!(next.as_mut()).is_pending());
    replies.unbounded_send(Ok(fixture.runner_response(1, StopReason::EndTurn)?))?;
    assert_eq!(next.await?.stop_reason, StopReason::EndTurn);
    Ok(())
}

#[tokio::test]
async fn cancellation_waits_for_permission_settlement_and_the_runners_final_update() -> TestResult {
    let fixture = Fixture::new(CancelDelivery::Accepted)?;
    let client = fixture.client()?;
    let pending = fixture.bridge.prompt(prompt());
    tokio::pin!(pending);
    assert!(futures::poll!(pending.as_mut()).is_pending());
    let permission = client.request_permission(RequestPermissionRequest::new(
        SESSION,
        ToolCallUpdate::new("tool-1", ToolCallUpdateFields::new()),
        vec![],
    ));
    tokio::pin!(permission);
    assert!(futures::poll!(permission.as_mut()).is_pending());

    fixture.bridge.cancel(CancelNotification::new(SESSION)).await?;
    assert!(futures::poll!(pending.as_mut()).is_pending());
    assert!(futures::poll!(permission.as_mut()).is_pending());
    fixture.settle_permission()?;
    assert!(matches!(permission.await?.outcome, RequestPermissionOutcome::Cancelled));
    client
        .session_notification(SessionNotification::new(
            SESSION,
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("Final update."))),
        ))
        .await?;
    let subjects = fixture.nats.client.published_messages();
    assert_eq!(subjects.last().ok_or("final update was not published")?, UPDATE);
    let payloads = fixture.nats.client.published_payloads();
    let body: serde_json::Value = serde_json::from_slice(payloads.last().ok_or("final update has no payload")?)?;
    assert_eq!(body["method"], "session/update");
    assert_eq!(body["params"]["sessionId"], SESSION);
    assert_eq!(body["params"]["update"]["content"]["text"], "Final update.");
    assert!(futures::poll!(pending.as_mut()).is_pending());
    fixture.settle_runner(0, StopReason::Cancelled)?;
    assert_eq!(pending.await?.stop_reason, StopReason::Cancelled);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cancellation_preserves_the_existing_prompt_timeout_when_the_runner_does_not_answer() -> TestResult {
    let fixture = Fixture::new(CancelDelivery::Accepted)?;
    let pending = fixture.bridge.prompt(prompt());
    tokio::pin!(pending);
    assert!(futures::poll!(pending.as_mut()).is_pending());
    fixture.bridge.cancel(CancelNotification::new(SESSION)).await?;
    assert!(futures::poll!(pending.as_mut()).is_pending());
    tokio::time::advance(PROMPT_TIMEOUT).await;
    let error = pending
        .await
        .err()
        .ok_or("prompt without runner settlement succeeded")?;
    assert_eq!(error.code, ErrorCode::INTERNAL_ERROR);
    assert_eq!(error.message, "prompt timed out waiting for runner");
    Ok(())
}
