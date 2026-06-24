use crate::client::{FlushClient, PublishClient, RequestClient};
use crate::telemetry::messaging::{
    MessagingError, MessagingOperation, set_client_operation_span_attributes, set_span_error,
};
use async_nats::header::HeaderMap;
use opentelemetry::propagation::Injector;
use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;
use tracing::{Span, instrument};
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::constants::{DEFAULT_TIMEOUT, REQ_ID_HEADER};

struct HeaderMapCarrier<'a>(&'a mut HeaderMap);

impl Injector for HeaderMapCarrier<'_> {
    fn set(&mut self, key: &str, value: String) {
        self.0.insert(key, value.as_str());
    }
}

pub fn inject_trace_context(headers: &mut HeaderMap) {
    let cx = Span::current().context();
    opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&cx, &mut HeaderMapCarrier(headers));
    });
}

pub fn headers_with_trace_context() -> HeaderMap {
    let mut headers = HeaderMap::new();
    inject_trace_context(&mut headers);
    headers
}

pub fn build_request_headers() -> HeaderMap {
    let mut headers = headers_with_trace_context();
    headers.insert(REQ_ID_HEADER, uuid::Uuid::new_v4().to_string().as_str());
    headers
}

#[instrument(name = "nats.request", skip(client, request), fields(subject = %subject))]
pub async fn request_with_timeout<N: RequestClient, Req, Res>(
    client: &N,
    subject: &str,
    request: &Req,
    timeout: Duration,
) -> Result<Res, NatsError>
where
    Req: Serialize,
    Res: DeserializeOwned,
{
    let span = Span::current();
    set_client_operation_span_attributes(&span, MessagingOperation::Request, subject);

    let payload = serde_json::to_vec(request).map_err(|error| {
        set_span_error(&span, MessagingError::Serialize);
        NatsError::Serialize(error)
    })?;
    let headers = build_request_headers();

    let response = tokio::time::timeout(
        timeout,
        client.request_with_headers(subject.to_string(), headers, payload.into()),
    )
    .await
    .map_err(|_| {
        set_span_error(&span, MessagingError::Timeout);
        NatsError::Timeout {
            subject: subject.to_string(),
        }
    })?
    .map_err(|error| {
        set_span_error(&span, MessagingError::Request);
        NatsError::Request {
            subject: subject.to_string(),
            error: error.to_string(),
        }
    })?;

    let payload_str = String::from_utf8_lossy(&response.payload);
    tracing::debug!(payload = %payload_str, "Received NATS response");

    serde_json::from_slice(&response.payload).map_err(|error| {
        set_span_error(&span, MessagingError::Deserialize);
        tracing::error!(
            error = %error,
            subject = %subject,
            "Failed to deserialize NATS response"
        );
        NatsError::Deserialize(error)
    })
}

pub async fn request<N: RequestClient, Req, Res>(client: &N, subject: &str, request: &Req) -> Result<Res, NatsError>
where
    Req: Serialize,
    Res: DeserializeOwned,
{
    request_with_timeout(client, subject, request, DEFAULT_TIMEOUT).await
}

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// Set to 0 to disable retries.
    pub max_retries: u32,
    /// Exponential backoff: delay * 2^retry_number.
    pub initial_retry_delay: Duration,
}

impl RetryPolicy {
    pub fn no_retries() -> Self {
        Self {
            max_retries: 0,
            initial_retry_delay: Duration::from_millis(50),
        }
    }

    pub fn standard() -> Self {
        Self {
            max_retries: 3,
            initial_retry_delay: Duration::from_millis(50),
        }
    }

    pub async fn execute<F, Fut>(&self, mut operation: F, operation_name: &str, subject: &str) -> Result<(), NatsError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<(), PublishOperationError>>,
    {
        let mut last_error = match operation().await {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };

        for attempt in 1..=self.max_retries {
            let exp = (attempt - 1).min(31);
            let delay = self.initial_retry_delay * (1u32 << exp);
            tracing::debug!(
                error = %last_error,
                operation = operation_name,
                subject = %subject,
                attempt,
                max_retries = self.max_retries,
                delay_ms = delay.as_millis(),
                "Operation failed, retrying"
            );
            tokio::time::sleep(delay).await;

            match operation().await {
                Ok(()) => {
                    tracing::info!(
                        operation = operation_name,
                        subject = %subject,
                        attempts = attempt + 1,
                        "Operation succeeded after retries"
                    );
                    return Ok(());
                }
                Err(e) => last_error = e,
            }
        }

        let attempts = self.max_retries + 1;
        if self.max_retries > 0 {
            tracing::warn!(
                error = %last_error,
                operation = operation_name,
                subject = %subject,
                total_attempts = attempts,
                "Operation failed after all retry attempts"
            );
            Err(NatsError::PublishOperationExhausted {
                error: last_error,
                subject: subject.to_string(),
                attempts,
            })
        } else {
            tracing::warn!(
                error = %last_error,
                operation = operation_name,
                subject = %subject,
                "Operation failed"
            );
            Err(NatsError::PublishOperation(last_error))
        }
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::no_retries()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FlushPolicy {
    pub retry_policy: RetryPolicy,
}

impl FlushPolicy {
    pub fn no_retries() -> Self {
        Self {
            retry_policy: RetryPolicy::no_retries(),
        }
    }

    pub fn standard() -> Self {
        Self {
            retry_policy: RetryPolicy::standard(),
        }
    }
}

impl Default for FlushPolicy {
    fn default() -> Self {
        Self::no_retries()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PublishOptions {
    pub publish_retry_policy: RetryPolicy,
    pub flush: Option<FlushPolicy>,
}

impl PublishOptions {
    pub fn simple() -> Self {
        Self::default()
    }

    pub fn builder() -> PublishOptionsBuilder {
        PublishOptionsBuilder::default()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PublishOptionsBuilder {
    publish_retry_policy: RetryPolicy,
    flush: Option<FlushPolicy>,
}

impl PublishOptionsBuilder {
    pub fn publish_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.publish_retry_policy = policy;
        self
    }

    pub fn flush_policy(mut self, policy: FlushPolicy) -> Self {
        self.flush = Some(policy);
        self
    }

    pub fn build(self) -> PublishOptions {
        PublishOptions {
            publish_retry_policy: self.publish_retry_policy,
            flush: self.flush,
        }
    }
}

#[instrument(name = "nats.publish", skip(client, request, options), fields(subject = %subject))]
pub async fn publish<N: PublishClient + FlushClient, Req>(
    client: &N,
    subject: &str,
    request: &Req,
    options: PublishOptions,
) -> Result<(), NatsError>
where
    Req: Serialize,
{
    let span = Span::current();
    set_client_operation_span_attributes(&span, MessagingOperation::Publish, subject);

    let payload = serde_json::to_vec(request).map_err(|error| {
        set_span_error(&span, MessagingError::Serialize);
        NatsError::Serialize(error)
    })?;
    let headers = headers_with_trace_context();

    options
        .publish_retry_policy
        .execute(
            || async {
                client
                    .publish_with_headers(subject.to_string(), headers.clone(), payload.clone().into())
                    .await
                    .map_err(|e| PublishOperationError(e.to_string()))
            },
            "publish",
            subject,
        )
        .await
        .inspect_err(|_error| {
            set_span_error(&span, MessagingError::PublishOperation);
        })?;

    let Some(flush_policy) = options.flush else {
        return Ok(());
    };

    flush_policy
        .retry_policy
        .execute(
            || {
                let client = client.clone();
                async move { client.flush().await.map_err(|e| PublishOperationError(e.to_string())) }
            },
            "flush",
            subject,
        )
        .await
        .inspect_err(|_error| {
            set_span_error(&span, MessagingError::FlushOperation);
        })
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct PublishOperationError(pub String);

#[derive(Debug, thiserror::Error)]
pub enum NatsError {
    #[error("Failed to serialize request: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("Failed to deserialize response: {0}")]
    Deserialize(#[source] serde_json::Error),
    #[error("Request to '{subject}' failed: {error}")]
    Request { subject: String, error: String },
    #[error("Publish operation failed: {0}")]
    PublishOperation(#[source] PublishOperationError),
    #[error("Publish operation failed after {attempts} attempts on '{subject}': {error}")]
    PublishOperationExhausted {
        #[source]
        error: PublishOperationError,
        subject: String,
        attempts: u32,
    },
    #[error("Request to '{subject}' timed out. The backend may be overloaded or unresponsive.")]
    Timeout { subject: String },
    #[error("NATS error: {0}")]
    Other(String),
}

#[cfg(test)]
mod tests;
