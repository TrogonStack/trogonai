use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_nats::header::{IntoHeaderValue, NATS_MESSAGE_ID};
use async_nats::HeaderMap;
use bytes::Bytes;

use crate::constants::HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS;
use crate::push::authentication_header::{AuthenticationHeaderBuildError, authorization_header_value};
use crate::push::delivery_semantics::DeliverySemantics;
use crate::push::nats_push_subject::NatsPushSubject;
use crate::push::push_idempotency_key::PushIdempotencyKey;
use crate::push::push_notification_config_id::{PushNotificationConfigId, PushNotificationConfigIdError};
use crate::push::push_notification_target::{PushNotificationTarget, PushNotificationTargetError};
use crate::push::target::WebhookUrlError;
use crate::push::terminal_push_task_state::TerminalPushTaskState;
use crate::task_id::A2aTaskId;

#[derive(Clone, Debug)]
pub enum DispatchPrepError {
    PushConfigId(PushNotificationConfigIdError),
}

impl fmt::Display for DispatchPrepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PushConfigId(inner) => std::fmt::Display::fmt(inner, f),
        }
    }
}

impl std::error::Error for DispatchPrepError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::PushConfigId(inner) => Some(inner),
        }
    }
}

#[derive(Debug)]
pub enum DispatchError {
    Prep(DispatchPrepError),
    InvalidTarget(PushNotificationTargetError),
    InvalidAuthorization(AuthenticationHeaderBuildError),
    InvalidHeader(Box<dyn std::error::Error + Send + Sync>),
    Http(reqwest::Error),
    UnexpectedStatus { status: u16, url: String },
    NatsPublish(NatsPublishDispatchError),
    JetStreamPublish(JetStreamPublishDispatchError),
}

#[derive(Debug)]
pub struct NatsPublishDispatchError {
    subject: String,
    source: Box<dyn std::error::Error + Send + Sync>,
}

#[derive(Debug)]
pub struct JetStreamPublishDispatchError {
    subject: String,
    source: Box<dyn std::error::Error + Send + Sync>,
}

impl NatsPublishDispatchError {
    fn new(subject: impl Into<String>, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            subject: subject.into(),
            source: Box::new(source),
        }
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }
}

impl JetStreamPublishDispatchError {
    fn new(subject: impl Into<String>, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            subject: subject.into(),
            source: Box::new(source),
        }
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }
}

impl fmt::Display for DispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Prep(inner) => std::fmt::Display::fmt(inner, f),
            Self::InvalidTarget(e) => write!(f, "invalid push notification URL: {e}"),
            Self::InvalidAuthorization(e) => write!(f, "invalid push notification authorization: {e}"),
            Self::InvalidHeader(e) => write!(f, "invalid push notification outbound header value: {e}"),
            Self::Http(e) => write!(f, "HTTP request failed: {e}"),
            Self::UnexpectedStatus { status, url } => {
                write!(f, "push notification to {url} returned status {status}")
            }
            Self::NatsPublish(e) => write!(f, "NATS publish to {} failed: {}", e.subject, e.source),
            Self::JetStreamPublish(e) => {
                write!(f, "JetStream publish to {} failed: {}", e.subject, e.source)
            }
        }
    }
}

impl fmt::Display for NatsPublishDispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "publish to {} failed: {}", self.subject, self.source)
    }
}

impl fmt::Display for JetStreamPublishDispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "publish to {} failed: {}", self.subject, self.source)
    }
}

impl std::error::Error for DispatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Prep(inner) => Some(inner),
            Self::InvalidTarget(e) => Some(e),
            Self::InvalidAuthorization(e) => Some(e),
            Self::InvalidHeader(e) => Some(&**e),
            Self::Http(e) => Some(e),
            Self::UnexpectedStatus { .. } => None,
            Self::NatsPublish(e) => Some(&*e.source),
            Self::JetStreamPublish(e) => Some(&*e.source),
        }
    }
}

impl std::error::Error for NatsPublishDispatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.source)
    }
}

impl std::error::Error for JetStreamPublishDispatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.source)
    }
}

impl From<PushNotificationTargetError> for DispatchError {
    fn from(e: PushNotificationTargetError) -> Self {
        Self::InvalidTarget(e)
    }
}

impl From<WebhookUrlError> for DispatchError {
    fn from(e: WebhookUrlError) -> Self {
        Self::InvalidTarget(PushNotificationTargetError::Http(e))
    }
}

fn webhook_http_retryable(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500 | 501 | 502 | 503 | 504 | 522 | 524)
}

fn push_transport_retryable(error: &reqwest::Error) -> bool {
    error.is_timeout() || error.is_connect()
}

pub(crate) fn maybe_terminal_push_idempotency_key(
    config: &a2a_types::TaskPushNotificationConfig,
    task_id: &A2aTaskId,
    semantics: &DeliverySemantics,
    terminal_state: TerminalPushTaskState,
) -> Result<Option<PushIdempotencyKey>, DispatchPrepError> {
    if !semantics.idempotency_key_required() {
        return Ok(None);
    }
    let cid = PushNotificationConfigId::new(config.id.clone()).map_err(DispatchPrepError::PushConfigId)?;
    Ok(Some(PushIdempotencyKey::derive_terminal(task_id, &cid, terminal_state)))
}

#[async_trait::async_trait]
pub trait PushDispatcher: Send + Sync + 'static {
    async fn dispatch(
        &self,
        task_id: &A2aTaskId,
        config: &a2a_types::TaskPushNotificationConfig,
        delivery_semantics: DeliverySemantics,
        terminal_task_state: TerminalPushTaskState,
        payload: &[u8],
    ) -> Result<(), DispatchError>;
}

pub struct HttpPushDispatcher {
    client: reqwest::Client,
}

impl HttpPushDispatcher {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl PushDispatcher for HttpPushDispatcher {
    async fn dispatch(
        &self,
        task_id: &A2aTaskId,
        config: &a2a_types::TaskPushNotificationConfig,
        delivery_semantics: DeliverySemantics,
        terminal_task_state: TerminalPushTaskState,
        payload: &[u8],
    ) -> Result<(), DispatchError> {
        let PushNotificationTarget::Http(url) = PushNotificationTarget::parse(&config.url)? else {
            return Err(DispatchError::InvalidTarget(PushNotificationTargetError::UnknownScheme {
                raw: config.url.clone(),
            }));
        };

        let authorization =
            authorization_header_value(config.authentication.as_ref()).map_err(DispatchError::InvalidAuthorization)?;

        let maybe_key =
            maybe_terminal_push_idempotency_key(config, task_id, &delivery_semantics, terminal_task_state).map_err(DispatchError::Prep)?;

        let max_attempts_usize = usize::try_from(delivery_semantics.webhook_max_publish_attempts()).unwrap_or(HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS as usize);

        let mut delay = std::time::Duration::from_millis(100);

        for attempt in 0..max_attempts_usize {
            let mut request = self
                .client
                .post(url.as_str())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(payload.to_vec());

            if let Some(ref auth_value) = authorization {
                request = request.header(reqwest::header::AUTHORIZATION, auth_value.clone());
            }

            if let Some(hn) = delivery_semantics.webhook_idempotency_carrier() {
                if let Some(ref k) = maybe_key {
                    let hv =
                        reqwest::header::HeaderValue::from_str(k.as_str()).map_err(|e| DispatchError::InvalidHeader(Box::new(e)))?;
                    request = request.header(hn.clone(), hv);
                }
            }

            match request.send().await {
                Ok(response) => {
                    if response.status().is_success() {
                        return Ok(());
                    }

                    let status = response.status().as_u16();
                    let should_retry = webhook_http_retryable(status)
                        && attempt + 1 < max_attempts_usize;
                    if should_retry {
                        tokio::time::sleep(delay).await;
                        delay = delay.saturating_mul(2).min(std::time::Duration::from_secs(3));
                        continue;
                    }

                    return Err(DispatchError::UnexpectedStatus {
                        status,
                        url: url.to_string(),
                    });
                }
                Err(e) => {
                    let should_retry =
                        push_transport_retryable(&e) && attempt + 1 < max_attempts_usize;
                    if should_retry {
                        tokio::time::sleep(delay).await;
                        delay = delay.saturating_mul(2).min(std::time::Duration::from_secs(3));
                        continue;
                    }
                    return Err(DispatchError::Http(e));
                }
            }
        }

        unreachable!("webhook retry loop invariant");
    }
}

pub struct NatsPublishPushDispatcher<N> {
    nats: N,
}

impl<N> NatsPublishPushDispatcher<N> {
    pub fn new(nats: N) -> Self {
        Self { nats }
    }
}

#[async_trait::async_trait]
impl<N> PushDispatcher for NatsPublishPushDispatcher<N>
where
    N: trogon_nats::PublishClient + Clone + Send + Sync + 'static,
{
    async fn dispatch(
        &self,
        task_id: &A2aTaskId,
        config: &a2a_types::TaskPushNotificationConfig,
        delivery_semantics: DeliverySemantics,
        terminal_task_state: TerminalPushTaskState,
        payload: &[u8],
    ) -> Result<(), DispatchError> {
        let PushNotificationTarget::Nats(subject) = PushNotificationTarget::parse(&config.url)? else {
            return Err(DispatchError::InvalidTarget(PushNotificationTargetError::UnknownScheme {
                raw: config.url.clone(),
            }));
        };

        let maybe_key =
            maybe_terminal_push_idempotency_key(config, task_id, &delivery_semantics, terminal_task_state).map_err(DispatchError::Prep)?;

        let nats_msg_id = maybe_key.as_ref().map(PushIdempotencyKey::as_str);

        let max_attempts = usize::try_from(delivery_semantics.nats_core_publish_max_attempts())
            .unwrap_or(HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS as usize);

        let mut delay = Duration::from_millis(100);
        for attempt in 0..max_attempts {
            match publish_json(&self.nats, &subject, payload, nats_msg_id).await {
                Ok(()) => return Ok(()),
                Err(e) if attempt + 1 < max_attempts => {
                    tokio::time::sleep(delay).await;
                    delay = delay.saturating_mul(2).min(Duration::from_secs(3));
                }
                Err(e) => return Err(e),
            }
        }

        unreachable!("nats retry loop invariant");
    }
}

pub struct JetStreamPublishPushDispatcher<J> {
    js: J,
}

impl<J> JetStreamPublishPushDispatcher<J> {
    pub fn new(js: J) -> Self {
        Self { js }
    }
}

#[async_trait::async_trait]
impl<J> PushDispatcher for JetStreamPublishPushDispatcher<J>
where
    J: trogon_nats::jetstream::JetStreamPublisher + Clone + Send + Sync + 'static,
{
    async fn dispatch(
        &self,
        task_id: &A2aTaskId,
        config: &a2a_types::TaskPushNotificationConfig,
        delivery_semantics: DeliverySemantics,
        terminal_task_state: TerminalPushTaskState,
        payload: &[u8],
    ) -> Result<(), DispatchError> {
        let PushNotificationTarget::JetStream(subject) = PushNotificationTarget::parse(&config.url)? else {
            return Err(DispatchError::InvalidTarget(PushNotificationTargetError::UnknownScheme {
                raw: config.url.clone(),
            }));
        };

        let maybe_key =
            maybe_terminal_push_idempotency_key(config, task_id, &delivery_semantics, terminal_task_state).map_err(DispatchError::Prep)?;

        let nats_msg_id = maybe_key.as_ref().map(PushIdempotencyKey::as_str);

        let max_attempts = usize::try_from(delivery_semantics.jetstream_max_publish_attempts())
            .unwrap_or(HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS as usize);

        let mut delay = Duration::from_millis(100);
        for attempt in 0..max_attempts {
            match publish_json_jetstream(&self.js, &subject, payload, nats_msg_id).await {
                Ok(()) => return Ok(()),
                Err(e) if attempt + 1 < max_attempts => {
                    tokio::time::sleep(delay).await;
                    delay = delay.saturating_mul(2).min(Duration::from_secs(3));
                }
                Err(e) => return Err(e),
            }
        }

        unreachable!("jetstream retry loop invariant");
    }
}

pub struct CompositePushDispatcher {
    http: HttpPushDispatcher,
    nats: Box<dyn PushDispatcher>,
    jetstream: Box<dyn PushDispatcher>,
}

impl CompositePushDispatcher {
    pub fn new(http: HttpPushDispatcher, nats: Box<dyn PushDispatcher>, jetstream: Box<dyn PushDispatcher>) -> Self {
        Self { http, nats, jetstream }
    }
}

#[async_trait::async_trait]
impl PushDispatcher for CompositePushDispatcher {
    async fn dispatch(
        &self,
        task_id: &A2aTaskId,
        config: &a2a_types::TaskPushNotificationConfig,
        delivery_semantics: DeliverySemantics,
        terminal_task_state: TerminalPushTaskState,
        payload: &[u8],
    ) -> Result<(), DispatchError> {
        match PushNotificationTarget::parse(&config.url)? {
            PushNotificationTarget::Http(_) => {
                self.http
                    .dispatch(task_id, config, delivery_semantics, terminal_task_state, payload)
                    .await
            }
            PushNotificationTarget::Nats(_) => {
                self.nats
                    .dispatch(task_id, config, delivery_semantics, terminal_task_state, payload)
                    .await
            }
            PushNotificationTarget::JetStream(_) => {
                self.jetstream
                    .dispatch(task_id, config, delivery_semantics, terminal_task_state, payload)
                    .await
            }
        }
    }
}

pub fn composite_push_dispatcher<N, J>(nats: N, jetstream: J, http_client: reqwest::Client) -> Arc<dyn PushDispatcher>
where
    N: trogon_nats::PublishClient + Clone + Send + Sync + 'static,
    J: trogon_nats::jetstream::JetStreamPublisher + Clone + Send + Sync + 'static,
{
    Arc::new(CompositePushDispatcher::new(
        HttpPushDispatcher::new(http_client),
        Box::new(NatsPublishPushDispatcher::new(nats)),
        Box::new(JetStreamPublishPushDispatcher::new(jetstream)),
    ))
}

async fn publish_json<N>(
    nats: &N,
    subject: &NatsPushSubject,
    payload: &[u8],
    nats_message_id: Option<&str>,
) -> Result<(), DispatchError>
where
    N: trogon_nats::PublishClient + Clone + Send + Sync + 'static,
{
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/json");
    if let Some(id) = nats_message_id {
        headers.insert(NATS_MESSAGE_ID, id.into_header_value());
    }
    nats.publish_with_headers(
        async_nats::Subject::from(subject.as_str()),
        headers,
        Bytes::copy_from_slice(payload),
    )
    .await
    .map_err(|e| DispatchError::NatsPublish(NatsPublishDispatchError::new(subject.as_str(), e)))
}

async fn publish_json_jetstream<J>(
    js: &J,
    subject: &NatsPushSubject,
    payload: &[u8],
    nats_message_id: Option<&str>,
) -> Result<(), DispatchError>
where
    J: trogon_nats::jetstream::JetStreamPublisher + Clone + Send + Sync + 'static,
{
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/json");
    if let Some(id) = nats_message_id {
        headers.insert(NATS_MESSAGE_ID, id.into_header_value());
    }

    let ack = js
        .publish_with_headers(
            async_nats::Subject::from(subject.as_str()),
            headers,
            Bytes::copy_from_slice(payload),
        )
        .await
        .map_err(|e| DispatchError::JetStreamPublish(JetStreamPublishDispatchError::new(subject.as_str(), e)))?;

    let ack_result = ack.await.map_err(|e| {
        DispatchError::JetStreamPublish(JetStreamPublishDispatchError::new(subject.as_str(), e))
    })?;

    if ack_result.duplicate {
        tracing::trace!(subject = subject.as_str(), "JetStream accepted duplicate Msg-Id terminal push ack");
    }
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_nats::jetstream::MockJetStreamPublisher;

    pub struct MockPushDispatcher {
        #[allow(clippy::type_complexity)]
        pub calls:
            Arc<Mutex<Vec<(A2aTaskId, DeliverySemantics, TerminalPushTaskState, a2a_types::TaskPushNotificationConfig, Vec<u8>)>>>,
        pub result: Arc<Mutex<Option<Result<(), String>>>>,
    }

    impl Default for MockPushDispatcher {
        fn default() -> Self {
            Self::new()
        }
    }

    impl MockPushDispatcher {
        pub fn new() -> Self {
            Self {
                calls: Arc::new(Mutex::new(vec![])),
                result: Arc::new(Mutex::new(None)),
            }
        }

        pub fn fail_with(error: impl Into<String>) -> Self {
            let d = Self::new();
            *d.result.lock().unwrap() = Some(Err(error.into()));
            d
        }

        #[allow(clippy::type_complexity)]
        pub fn recorded_calls(
            &self,
        ) -> Vec<(
            A2aTaskId,
            DeliverySemantics,
            TerminalPushTaskState,
            a2a_types::TaskPushNotificationConfig,
            Vec<u8>,
        )> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl PushDispatcher for MockPushDispatcher {
        async fn dispatch(
            &self,
            task_id: &A2aTaskId,
            config: &a2a_types::TaskPushNotificationConfig,
            delivery_semantics: DeliverySemantics,
            terminal_task_state: TerminalPushTaskState,
            payload: &[u8],
        ) -> Result<(), DispatchError> {
            self.calls.lock().unwrap().push((
                task_id.clone(),
                delivery_semantics,
                terminal_task_state,
                config.clone(),
                payload.to_vec(),
            ));
            match self.result.lock().unwrap().take() {
                Some(Err(msg)) => Err(DispatchError::UnexpectedStatus { status: 500, url: msg }),
                _ => Ok(()),
            }
        }
    }

    fn push_config(url: &str) -> a2a_types::TaskPushNotificationConfig {
        a2a_types::TaskPushNotificationConfig {
            id: "cfg-1".to_string(),
            task_id: "task-99".into(),
            url: url.to_string(),
            ..Default::default()
        }
    }

    fn task_id_fix() -> A2aTaskId {
        A2aTaskId::new("task-99").unwrap()
    }

    fn terminal_completed() -> TerminalPushTaskState {
        TerminalPushTaskState::Completed
    }

    #[test]
    fn error_display_invalid_target() {
        let err = DispatchError::InvalidTarget(PushNotificationTarget::parse("ftp://bad").unwrap_err());
        assert!(err.to_string().contains("invalid push notification URL"));
    }

    #[test]
    fn error_display_invalid_authorization() {
        let err = DispatchError::InvalidAuthorization(
            crate::push::authentication_header::AuthenticationHeaderBuildError::UnsupportedScheme {
                scheme: "digest".into(),
            },
        );
        assert!(err.to_string().contains("invalid push notification authorization"));
    }

    #[test]
    fn error_display_unexpected_status() {
        let err = DispatchError::UnexpectedStatus {
            status: 429,
            url: "https://example.com".into(),
        };
        assert!(err.to_string().contains("429"));
        assert!(err.to_string().contains("https://example.com"));
    }

    #[tokio::test]
    async fn nats_dispatcher_publishes_json_payload_to_subject() {
        let nats = AdvancedMockNatsClient::new();
        let dispatcher = NatsPublishPushDispatcher::new(nats.clone());
        let payload = br#"{"event":"completed"}"#;
        let config = push_config("subject:a2a.push.acme.caller-42.task-9");
        dispatcher
            .dispatch(&task_id_fix(), &config, DeliverySemantics::AtLeastOnce, terminal_completed(), payload)
            .await
            .unwrap();

        assert_eq!(nats.published_messages(), vec!["a2a.push.acme.caller-42.task-9"]);
        assert_eq!(nats.published_payloads()[0].as_ref(), &payload[..]);
    }

    #[tokio::test]
    async fn nats_dispatcher_retries_publish_after_mock_failures() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_publish_count(2);
        let dispatcher = NatsPublishPushDispatcher::new(nats.clone());
        let payload = br#"{"event":"completed"}"#;
        let config = push_config("subject:a2a.push.acme.caller.retry");

        dispatcher
            .dispatch(&task_id_fix(), &config, DeliverySemantics::AtLeastOnce, terminal_completed(), payload)
            .await
            .unwrap();

        assert_eq!(nats.published_messages(), vec!["a2a.push.acme.caller.retry"]);
        assert_eq!(nats.published_payloads()[0].as_ref(), &payload[..]);
    }

    #[tokio::test]
    async fn nats_dispatcher_stops_after_max_publish_failures() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_publish_count(crate::constants::HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS);
        let dispatcher = NatsPublishPushDispatcher::new(nats.clone());
        let config = push_config("subject:a2a.push.acme.caller.fail");

        let err = dispatcher
            .dispatch(&task_id_fix(), &config, DeliverySemantics::AtLeastOnce, terminal_completed(), br#"{}"#)
            .await
            .unwrap_err();
        assert!(matches!(err, DispatchError::NatsPublish(_)));
        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn nats_dispatcher_rejects_http_url() {
        let nats = AdvancedMockNatsClient::new();
        let dispatcher = NatsPublishPushDispatcher::new(nats.clone());
        let config = push_config("https://example.com/hook");

        let err = dispatcher
            .dispatch(&task_id_fix(), &config, DeliverySemantics::AtLeastOnce, terminal_completed(), b"{}")
            .await
            .unwrap_err();
        assert!(matches!(err, DispatchError::InvalidTarget(_)));
        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn nats_exactly_once_single_attempt_when_publish_fails() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_publish_count(10);
        let dispatcher = NatsPublishPushDispatcher::new(nats.clone());
        let config = push_config("subject:a2a.fail");

        let err = dispatcher
            .dispatch(
                &task_id_fix(),
                &config,
                DeliverySemantics::ExactlyOnce {
                    idempotency_key_header: None,
                },
                TerminalPushTaskState::Completed,
                b"{}",
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DispatchError::NatsPublish(_)));
        assert!(nats.published_messages().is_empty());
    }

    fn composite(
        http_client: reqwest::Client,
        nats: AdvancedMockNatsClient,
        js: MockJetStreamPublisher,
    ) -> CompositePushDispatcher {
        CompositePushDispatcher::new(
            HttpPushDispatcher::new(http_client),
            Box::new(NatsPublishPushDispatcher::new(nats)),
            Box::new(JetStreamPublishPushDispatcher::new(js)),
        )
    }

    #[tokio::test]
    async fn jetstream_dispatcher_publishes_json_payload_to_subject() {
        let js = MockJetStreamPublisher::new();
        let dispatcher = JetStreamPublishPushDispatcher::new(js.clone());
        let payload = br#"{"event":"completed"}"#;
        let config = push_config("jetstream:a2a.push.acme.caller-42.task-9");

        dispatcher
            .dispatch(&task_id_fix(), &config, DeliverySemantics::AtLeastOnce, terminal_completed(), payload)
            .await
            .unwrap();

        assert_eq!(js.published_subjects(), vec!["a2a.push.acme.caller-42.task-9"]);
        assert_eq!(js.published_payloads()[0].as_ref(), &payload[..]);
    }

    #[tokio::test]
    async fn jetstream_dispatcher_retries_publish_after_mock_failures() {
        let js = MockJetStreamPublisher::new();
        js.fail_js_publish_count(2);
        let dispatcher = JetStreamPublishPushDispatcher::new(js.clone());
        let payload = br#"{"event":"completed"}"#;
        let config = push_config("jetstream:a2a.push.acme.caller.retry");

        dispatcher
            .dispatch(&task_id_fix(), &config, DeliverySemantics::AtLeastOnce, terminal_completed(), payload)
            .await
            .unwrap();

        assert_eq!(js.published_subjects(), vec!["a2a.push.acme.caller.retry"]);
        assert_eq!(js.published_payloads()[0].as_ref(), &payload[..]);
    }

    #[tokio::test]
    async fn jetstream_dispatcher_stops_after_max_publish_failures() {
        let js = MockJetStreamPublisher::new();
        js.fail_js_publish_count(crate::constants::HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS);
        let dispatcher = JetStreamPublishPushDispatcher::new(js.clone());
        let config = push_config("jetstream:a2a.push.acme.caller.fail");

        let err = dispatcher
            .dispatch(&task_id_fix(), &config, DeliverySemantics::AtLeastOnce, terminal_completed(), br#"{}"#)
            .await
            .unwrap_err();
        assert!(matches!(err, DispatchError::JetStreamPublish(_)));
        assert!(js.published_subjects().is_empty());
    }

    #[tokio::test]
    async fn jetstream_dispatcher_rejects_nats_subject_url() {
        let js = MockJetStreamPublisher::new();
        let dispatcher = JetStreamPublishPushDispatcher::new(js.clone());
        let config = push_config("subject:a2a.push.acme.caller.task");

        let err = dispatcher
            .dispatch(&task_id_fix(), &config, DeliverySemantics::AtLeastOnce, terminal_completed(), b"{}")
            .await
            .unwrap_err();
        assert!(matches!(err, DispatchError::InvalidTarget(_)));
        assert!(js.published_subjects().is_empty());
    }

    #[tokio::test]
    async fn composite_routes_nats_target_to_nats_dispatcher() {
        let nats = AdvancedMockNatsClient::new();
        let js = MockJetStreamPublisher::new();
        let composite = composite(reqwest::Client::new(), nats.clone(), js);
        let payload = br#"{"task":"done"}"#;
        let config = push_config("subject:a2a.push.tenant.caller.task");

        composite
            .dispatch(&task_id_fix(), &config, DeliverySemantics::AtLeastOnce, terminal_completed(), payload)
            .await
            .unwrap();

        assert_eq!(nats.published_messages(), vec!["a2a.push.tenant.caller.task"]);
        assert_eq!(nats.published_payloads()[0].as_ref(), &payload[..]);
    }

    #[tokio::test]
    async fn composite_routes_jetstream_target_to_jetstream_dispatcher() {
        let nats = AdvancedMockNatsClient::new();
        let js = MockJetStreamPublisher::new();
        let composite = composite(reqwest::Client::new(), nats.clone(), js.clone());
        let payload = br#"{"task":"done"}"#;
        let config = push_config("jetstream:a2a.push.tenant.caller.task");

        composite
            .dispatch(&task_id_fix(), &config, DeliverySemantics::AtLeastOnce, terminal_completed(), payload)
            .await
            .unwrap();

        assert_eq!(js.published_subjects(), vec!["a2a.push.tenant.caller.task"]);
        assert_eq!(js.published_payloads()[0].as_ref(), &payload[..]);
        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn composite_rejects_unknown_scheme() {
        let nats = AdvancedMockNatsClient::new();
        let js = MockJetStreamPublisher::new();
        let composite = composite(reqwest::Client::new(), nats.clone(), js);
        let config = push_config("nats://broker");

        let err = composite
            .dispatch(&task_id_fix(), &config, DeliverySemantics::AtLeastOnce, terminal_completed(), b"{}")
            .await
            .unwrap_err();
        assert!(matches!(err, DispatchError::InvalidTarget(_)));
    }

    #[test]
    fn composite_push_dispatcher_helper_returns_trait_object() {
        let nats = AdvancedMockNatsClient::new();
        let js = MockJetStreamPublisher::new();
        let dispatcher = composite_push_dispatcher(nats, js, reqwest::Client::new());
        let _: Arc<dyn PushDispatcher> = dispatcher;
    }
}
