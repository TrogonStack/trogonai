use std::fmt;
use std::time::Duration;

use axum::{
    Router, body::Bytes, extract::DefaultBodyLimit, extract::State, http::HeaderMap, http::StatusCode, routing::post,
};
use tracing::{info, instrument, warn};
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut, PublishOutcome,
};
use trogon_std::{EmptySecret, NonZeroDuration};

use crate::runtime_projection::{RuntimeCredentialError, RuntimeCredentialResolver, RuntimeIntegrationKey};
use crate::secret_store::{CredentialKind, SecretStoreError, SecretStoreGet, SourceKind};
use crate::source_integration_id::SourceIntegrationId;

use super::NotionEventType;
use super::NotionVerificationToken;
use super::config::NotionConfig;
use super::constants::{
    HEADER_SIGNATURE, HTTP_BODY_SIZE_MAX, NATS_HEADER_ATTEMPT_NUMBER, NATS_HEADER_EVENT_ID, NATS_HEADER_EVENT_TYPE,
    NATS_HEADER_REJECT_REASON, NATS_HEADER_SUBSCRIPTION_ID,
};
use super::signature;
use super::verification_token::verification_subject;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RejectReason {
    InvalidJson,
    MissingEventType,
    InvalidEventType,
}

impl RejectReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::InvalidJson => "invalid_json",
            Self::MissingEventType => "missing_event_type",
            Self::InvalidEventType => "invalid_event_type",
        }
    }
}

#[derive(Clone, Debug, Default)]
struct EventMetadata {
    event_id: Option<String>,
    subscription_id: Option<String>,
    attempt_number: Option<u64>,
}

impl EventMetadata {
    fn from_value(value: &serde_json::Value) -> Self {
        Self {
            event_id: value.get("id").and_then(serde_json::Value::as_str).map(str::to_owned),
            subscription_id: value
                .get("subscription_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            attempt_number: value.get("attempt_number").and_then(serde_json::Value::as_u64),
        }
    }

    fn apply_headers(&self, headers: &mut async_nats::HeaderMap) {
        if let Some(ref event_id) = self.event_id {
            headers.insert(async_nats::header::NATS_MESSAGE_ID, event_id.as_str());
            headers.insert(NATS_HEADER_EVENT_ID, event_id.as_str());
        }
        if let Some(ref subscription_id) = self.subscription_id {
            headers.insert(NATS_HEADER_SUBSCRIPTION_ID, subscription_id.as_str());
        }
        if let Some(attempt_number) = self.attempt_number {
            let attempt_number = attempt_number.to_string();
            headers.insert(NATS_HEADER_ATTEMPT_NUMBER, attempt_number.as_str());
        }
    }
}

struct VerificationRequest {
    verification_token: NotionVerificationToken,
}

#[derive(Debug, thiserror::Error)]
enum VerificationRequestParseError {
    #[error("verification_token must not be empty")]
    InvalidVerificationToken(#[source] EmptySecret),
}

fn outcome_to_status<E: fmt::Display>(outcome: PublishOutcome<E>) -> StatusCode {
    if outcome.is_ok() {
        info!("Published Notion event to NATS");
        StatusCode::OK
    } else {
        outcome.log_on_error("notion");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

async fn publish_unroutable<P: JetStreamPublisher, S: ObjectStorePut>(
    publisher: &ClaimCheckPublisher<P, S>,
    subject_prefix: &NatsToken,
    metadata: &EventMetadata,
    reason: RejectReason,
    body: Bytes,
    ack_timeout: NonZeroDuration,
) -> StatusCode {
    let subject = format!("{subject_prefix}.unroutable");
    let mut headers = async_nats::HeaderMap::new();
    metadata.apply_headers(&mut headers);
    headers.insert(NATS_HEADER_REJECT_REASON, reason.as_str());

    let outcome = publisher
        .publish_event(subject, headers, body, ack_timeout.into())
        .await;

    if outcome.is_ok() {
        StatusCode::OK
    } else {
        outcome.log_on_error("notion.unroutable");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

async fn publish_verification<P: JetStreamPublisher, S: ObjectStorePut>(
    publisher: &ClaimCheckPublisher<P, S>,
    subject_prefix: &NatsToken,
    body: Bytes,
    ack_timeout: NonZeroDuration,
) -> StatusCode {
    let subject = verification_subject(subject_prefix);
    let outcome = publisher
        .publish_event(subject, async_nats::HeaderMap::new(), body, ack_timeout.into())
        .await;

    if outcome.is_ok() {
        info!("Published Notion subscription verification request to NATS");
        StatusCode::OK
    } else {
        outcome.log_on_error("notion.subscription.verification");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher, S: ObjectStorePut> {
    publisher: ClaimCheckPublisher<P, S>,
    verification_token: NotionVerificationToken,
    subject_prefix: NatsToken,
    nats_ack_timeout: NonZeroDuration,
}

#[derive(Clone)]
struct RuntimeAppState<P: JetStreamPublisher, S: ObjectStorePut, G> {
    publisher: ClaimCheckPublisher<P, S>,
    credential_resolver: RuntimeCredentialResolver<G>,
    runtime_key: RuntimeIntegrationKey,
    subject_prefix: NatsToken,
    nats_ack_timeout: NonZeroDuration,
}

pub async fn provision<C: JetStreamContext>(js: &C, config: &NotionConfig) -> Result<(), C::Error> {
    js.get_or_create_stream(async_nats::jetstream::stream::Config {
        name: config.stream_name.to_string(),
        subjects: vec![format!("{}.>", config.subject_prefix)],
        max_age: config.stream_max_age.into(),
        ..Default::default()
    })
    .await?;

    let max_age_secs = Duration::from(config.stream_max_age).as_secs();
    let stream_name = config.stream_name.as_str();
    info!(stream = stream_name, max_age_secs, "JetStream stream ready");
    Ok(())
}

pub fn router<P: JetStreamPublisher, S: ObjectStorePut>(
    publisher: ClaimCheckPublisher<P, S>,
    config: &NotionConfig,
) -> Router {
    let state = AppState {
        publisher,
        verification_token: config.verification_token.clone(),
        subject_prefix: config.subject_prefix.clone(),
        nats_ack_timeout: config.nats_ack_timeout,
    };

    Router::new()
        .route("/webhook", post(handle_webhook::<P, S>))
        .layer(DefaultBodyLimit::max(HTTP_BODY_SIZE_MAX.as_usize()))
        .with_state(state)
}

pub fn runtime_router<P, S, G>(
    publisher: ClaimCheckPublisher<P, S>,
    config: &NotionConfig,
    integration_id: SourceIntegrationId,
    credential_resolver: RuntimeCredentialResolver<G>,
) -> Router
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    G: SecretStoreGet<Error = SecretStoreError>,
{
    let state = RuntimeAppState {
        publisher,
        credential_resolver,
        runtime_key: RuntimeIntegrationKey::new(SourceKind::Notion, &integration_id),
        subject_prefix: config.subject_prefix.clone(),
        nats_ack_timeout: config.nats_ack_timeout,
    };

    Router::new()
        .route("/webhook", post(handle_runtime_webhook::<P, S, G>))
        .layer(DefaultBodyLimit::max(HTTP_BODY_SIZE_MAX.as_usize()))
        .with_state(state)
}

fn parse_verification_request(body: &[u8]) -> Result<Option<VerificationRequest>, VerificationRequestParseError> {
    let value: serde_json::Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    if value.get("type").is_some() {
        return Ok(None);
    }

    let Some(verification_token) = value.get("verification_token").and_then(|v| v.as_str()) else {
        return Ok(None);
    };

    let verification_token = NotionVerificationToken::new(verification_token)
        .map_err(VerificationRequestParseError::InvalidVerificationToken)?;

    Ok(Some(VerificationRequest { verification_token }))
}

#[instrument(
    name = "notion.webhook",
    skip_all,
    fields(
        event_type = tracing::field::Empty,
        event_id = tracing::field::Empty,
        subscription_id = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_webhook<P: JetStreamPublisher, S: ObjectStorePut>(
    State(state): State<AppState<P, S>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let signature = headers.get(HEADER_SIGNATURE).and_then(|value| value.to_str().ok());
    let span = tracing::Span::current();

    match parse_verification_request(&body) {
        Ok(Some(verification_request)) => {
            if let Some(signature) = signature
                && let Err(err) = signature::verify(&verification_request.verification_token, &body, signature)
            {
                warn!(error = %err, "Notion verification request signature validation failed");
                return StatusCode::UNAUTHORIZED;
            }

            let subject = verification_subject(&state.subject_prefix);
            span.record("event_type", "subscription.verification");
            span.record("subject", &subject);

            return publish_verification(&state.publisher, &state.subject_prefix, body, state.nats_ack_timeout).await;
        }
        Ok(None) => {}
        Err(err) => {
            warn!(error = %err, "Invalid Notion verification request payload");
            return StatusCode::BAD_REQUEST;
        }
    }

    match signature {
        Some(signature) => {
            if let Err(err) = signature::verify(&state.verification_token, &body, signature) {
                warn!(error = %err, "Notion webhook signature validation failed");
                return StatusCode::UNAUTHORIZED;
            }
        }
        None => {
            warn!("Missing X-Notion-Signature header");
            return StatusCode::UNAUTHORIZED;
        }
    }

    publish_verified_webhook(state.publisher, state.subject_prefix, state.nats_ack_timeout, body).await
}

#[instrument(
    name = "notion.webhook",
    skip_all,
    fields(
        event_type = tracing::field::Empty,
        event_id = tracing::field::Empty,
        subscription_id = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_runtime_webhook<P, S, G>(
    State(state): State<RuntimeAppState<P, S, G>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    G: SecretStoreGet<Error = SecretStoreError>,
{
    let signature = headers.get(HEADER_SIGNATURE).and_then(|value| value.to_str().ok());
    let span = tracing::Span::current();

    match parse_verification_request(&body) {
        Ok(Some(verification_request)) => {
            if let Some(signature) = signature
                && let Err(err) = signature::verify(&verification_request.verification_token, &body, signature)
            {
                warn!(error = %err, "Notion verification request signature validation failed");
                return StatusCode::UNAUTHORIZED;
            }

            let subject = verification_subject(&state.subject_prefix);
            span.record("event_type", "subscription.verification");
            span.record("subject", &subject);

            return publish_verification(&state.publisher, &state.subject_prefix, body, state.nats_ack_timeout).await;
        }
        Ok(None) => {}
        Err(err) => {
            warn!(error = %err, "Invalid Notion verification request payload");
            return StatusCode::BAD_REQUEST;
        }
    }

    let verification_token = match state
        .credential_resolver
        .resolve_plaintext(&state.runtime_key, CredentialKind::VerificationToken)
        .await
    {
        Ok(secret) => match NotionVerificationToken::new(secret.as_str()) {
            Ok(token) => token,
            Err(error) => {
                warn!(reason = %error, "Notion runtime verification token is invalid");
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
        },
        Err(error) => {
            warn!(reason = %error, "Notion runtime credential resolution failed");
            return runtime_credential_error_to_status(&error);
        }
    };

    match signature {
        Some(signature) => {
            if let Err(err) = signature::verify(&verification_token, &body, signature) {
                warn!(error = %err, "Notion webhook signature validation failed");
                return StatusCode::UNAUTHORIZED;
            }
        }
        None => {
            warn!("Missing X-Notion-Signature header");
            return StatusCode::UNAUTHORIZED;
        }
    }

    publish_verified_webhook(state.publisher, state.subject_prefix, state.nats_ack_timeout, body).await
}

async fn publish_verified_webhook<P: JetStreamPublisher, S: ObjectStorePut>(
    publisher: ClaimCheckPublisher<P, S>,
    subject_prefix: NatsToken,
    nats_ack_timeout: NonZeroDuration,
    body: Bytes,
) -> StatusCode {
    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(err) => {
            warn!(error = %err, "Failed to parse Notion webhook body as JSON");
            return publish_unroutable(
                &publisher,
                &subject_prefix,
                &EventMetadata::default(),
                RejectReason::InvalidJson,
                body,
                nats_ack_timeout,
            )
            .await;
        }
    };

    let metadata = EventMetadata::from_value(&parsed);
    let Some(raw_event_type) = parsed.get("type").and_then(serde_json::Value::as_str) else {
        warn!("Missing type in Notion webhook payload");
        return publish_unroutable(
            &publisher,
            &subject_prefix,
            &metadata,
            RejectReason::MissingEventType,
            body,
            nats_ack_timeout,
        )
        .await;
    };

    let event_type = match NotionEventType::new(raw_event_type) {
        Ok(event_type) => event_type,
        Err(err) => {
            warn!(error = %err, "Invalid type in Notion webhook payload");
            return publish_unroutable(
                &publisher,
                &subject_prefix,
                &metadata,
                RejectReason::InvalidEventType,
                body,
                nats_ack_timeout,
            )
            .await;
        }
    };

    let subject = format!("{}.{}", subject_prefix, event_type);
    let span = tracing::Span::current();
    span.record("event_type", event_type.as_str());
    span.record("event_id", metadata.event_id.as_deref().unwrap_or("unknown"));
    span.record(
        "subscription_id",
        metadata.subscription_id.as_deref().unwrap_or("unknown"),
    );
    span.record("subject", &subject);

    let mut nats_headers = async_nats::HeaderMap::new();
    metadata.apply_headers(&mut nats_headers);
    nats_headers.insert(NATS_HEADER_EVENT_TYPE, event_type.as_str());

    let outcome = publisher
        .publish_event(subject, nats_headers, body, nats_ack_timeout.into())
        .await;

    outcome_to_status(outcome)
}

fn runtime_credential_error_to_status(error: &RuntimeCredentialError) -> StatusCode {
    match error {
        RuntimeCredentialError::SecretStore(SecretStoreError::BackendUnavailable { .. }) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
        RuntimeCredentialError::SecretStore(_)
        | RuntimeCredentialError::IntegrationNotFound { .. }
        | RuntimeCredentialError::IntegrationNotResolvable { .. }
        | RuntimeCredentialError::CredentialMissing { .. }
        | RuntimeCredentialError::VerifierOnly { .. } => StatusCode::UNAUTHORIZED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;
    use tower::ServiceExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use trogon_nats::jetstream::StreamMaxAge;
    use trogon_nats::jetstream::{
        ClaimCheckPublisher, MaxPayload, MockJetStreamContext, MockJetStreamPublisher, MockObjectStore,
    };

    use crate::runtime_projection::{RuntimeCredentialRegistry, RuntimeIntegrationProjection};
    use crate::secret_store::{
        CredentialKind, CredentialOwnerId, CredentialScope, MockOpenBaoSecretStore, SecretStorePut, SourceKind,
    };
    use crate::source_integration_id::SourceIntegrationId;

    type HmacSha256 = Hmac<Sha256>;

    const TEST_VERIFICATION_TOKEN: &str = "notion-verification-token-example";

    fn wrap_publisher(
        publisher: MockJetStreamPublisher,
    ) -> ClaimCheckPublisher<MockJetStreamPublisher, MockObjectStore> {
        ClaimCheckPublisher::new(
            publisher,
            MockObjectStore::new(),
            "test-bucket".to_string(),
            MaxPayload::from_server_limit(usize::MAX),
        )
    }

    fn compute_sig(secret: &NotionVerificationToken, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_str().as_bytes()).unwrap();
        mac.update(body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    fn test_config() -> NotionConfig {
        NotionConfig {
            verification_token: NotionVerificationToken::new(TEST_VERIFICATION_TOKEN).unwrap(),
            subject_prefix: NatsToken::new("notion").unwrap(),
            stream_name: NatsToken::new("NOTION").unwrap(),
            stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
            nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        }
    }

    fn tracing_guard() -> tracing::subscriber::DefaultGuard {
        tracing_subscriber::fmt().with_test_writer().set_default()
    }

    fn mock_app(publisher: MockJetStreamPublisher) -> Router {
        router(wrap_publisher(publisher), &test_config())
    }

    async fn runtime_app(publisher: MockJetStreamPublisher, secret: &str) -> Router {
        let integration_id = SourceIntegrationId::new("primary").unwrap();
        let store = MockOpenBaoSecretStore::default();
        let credential = store
            .put(
                CredentialScope::integration(
                    CredentialOwnerId::new("tenant-1").unwrap(),
                    SourceKind::Notion,
                    integration_id.clone(),
                ),
                CredentialKind::VerificationToken,
                trogon_std::SecretString::new(secret).unwrap(),
            )
            .await
            .unwrap();
        let registry = RuntimeCredentialRegistry::default();
        registry
            .projections()
            .upsert(RuntimeIntegrationProjection::active_from_credential_ref(credential, 1).unwrap())
            .await;

        runtime_router(
            wrap_publisher(publisher),
            &test_config(),
            integration_id,
            registry.resolver(store),
        )
    }

    fn webhook_request(body: &[u8], signature: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().method("POST").uri("/webhook");

        if let Some(signature) = signature {
            builder = builder.header(HEADER_SIGNATURE, signature);
        }

        builder.body(Body::from(body.to_vec())).unwrap()
    }

    fn valid_event_body() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "id": "367cba44-b6f3-4c92-81e7-6a2e9659efd4",
            "subscription_id": "29d75c0d-5546-4414-8459-7b7a92f1fc4b",
            "attempt_number": 1,
            "type": "page.created",
            "entity": {
                "id": "153104cd-477e-809d-8dc4-ff2d96ae3090",
                "type": "page"
            }
        }))
        .unwrap()
    }

    fn verification_body() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "verification_token": TEST_VERIFICATION_TOKEN
        }))
        .unwrap()
    }

    fn assert_no_publishes(publisher: &MockJetStreamPublisher) {
        assert!(publisher.published_messages().is_empty());
    }

    fn assert_unroutable(publisher: &MockJetStreamPublisher, expected_reason: &str) {
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "notion.unroutable");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_REJECT_REASON)
                .map(|value| value.as_str()),
            Some(expected_reason),
        );
    }

    #[tokio::test]
    async fn provision_creates_stream() {
        let _guard = tracing_guard();
        let js = MockJetStreamContext::new();
        let config = test_config();

        provision(&js, &config).await.unwrap();

        let streams = js.created_streams();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].name, "NOTION");
        assert_eq!(streams[0].subjects, vec!["notion.>"]);
        assert_eq!(streams[0].max_age, Duration::from_secs(3600));
    }

    #[tokio::test]
    async fn valid_event_publishes_to_nats() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = valid_event_body();
        let signature = compute_sig(&test_config().verification_token, &body);

        let response = app.oneshot(webhook_request(&body, Some(&signature))).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "notion.page.created");
        assert_eq!(
            messages[0]
                .headers
                .get(async_nats::header::NATS_MESSAGE_ID)
                .map(|value| value.as_str()),
            Some("367cba44-b6f3-4c92-81e7-6a2e9659efd4"),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_EVENT_TYPE)
                .map(|value| value.as_str()),
            Some("page.created"),
        );
    }

    #[tokio::test]
    async fn runtime_verification_token_publishes_to_nats_and_rejects_static_secret() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = runtime_app(publisher.clone(), "runtime-secret").await;
        let body = valid_event_body();
        let static_signature = compute_sig(&test_config().verification_token, &body);

        let static_response = app
            .clone()
            .oneshot(webhook_request(&body, Some(&static_signature)))
            .await
            .unwrap();

        assert_eq!(static_response.status(), StatusCode::UNAUTHORIZED);
        assert_no_publishes(&publisher);

        let runtime_token = NotionVerificationToken::new("runtime-secret").unwrap();
        let runtime_signature = compute_sig(&runtime_token, &body);
        let runtime_response = app
            .oneshot(webhook_request(&body, Some(&runtime_signature)))
            .await
            .unwrap();

        assert_eq!(runtime_response.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "notion.page.created");
    }

    #[tokio::test]
    async fn bootstrap_verification_request_without_signature_is_accepted() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = verification_body();

        let response = app.oneshot(webhook_request(&body, None)).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(publisher.published_subjects(), vec!["notion.subscription.verification"]);
    }

    #[tokio::test]
    async fn verification_request_with_bad_signature_is_rejected() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = verification_body();

        let response = app.oneshot(webhook_request(&body, Some("sha256=bad"))).await.unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_no_publishes(&publisher);
    }

    #[tokio::test]
    async fn verification_request_with_empty_token_returns_400() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = serde_json::to_vec(&serde_json::json!({
            "verification_token": ""
        }))
        .unwrap();

        let response = app.oneshot(webhook_request(&body, None)).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_no_publishes(&publisher);
    }

    #[tokio::test]
    async fn missing_signature_returns_401() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = valid_event_body();

        let response = app.oneshot(webhook_request(&body, None)).await.unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_no_publishes(&publisher);
    }

    #[tokio::test]
    async fn invalid_json_publishes_unroutable_and_returns_ok() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = b"not-json";
        let signature = compute_sig(&test_config().verification_token, body);

        let response = app.oneshot(webhook_request(body, Some(&signature))).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_unroutable(&publisher, "invalid_json");
    }

    #[tokio::test]
    async fn missing_event_type_publishes_unroutable_and_returns_ok() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = serde_json::to_vec(&serde_json::json!({
            "id": "367cba44-b6f3-4c92-81e7-6a2e9659efd4"
        }))
        .unwrap();
        let signature = compute_sig(&test_config().verification_token, &body);

        let response = app.oneshot(webhook_request(&body, Some(&signature))).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_unroutable(&publisher, "missing_event_type");
    }

    #[tokio::test]
    async fn invalid_event_type_publishes_unroutable_and_returns_ok() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = serde_json::to_vec(&serde_json::json!({
            "id": "367cba44-b6f3-4c92-81e7-6a2e9659efd4",
            "type": "page created"
        }))
        .unwrap();
        let signature = compute_sig(&test_config().verification_token, &body);

        let response = app.oneshot(webhook_request(&body, Some(&signature))).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_unroutable(&publisher, "invalid_event_type");
    }

    #[tokio::test]
    async fn publish_failure_returns_500() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        publisher.fail_next_js_publish();
        let app = mock_app(publisher);
        let body = valid_event_body();
        let signature = compute_sig(&test_config().verification_token, &body);

        let response = app.oneshot(webhook_request(&body, Some(&signature))).await.unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
