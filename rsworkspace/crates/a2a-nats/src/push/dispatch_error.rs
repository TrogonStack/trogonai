//! Error types for the push dispatcher.
//!
//! Extracted into its own module so `dlq.rs` (and other consumers of
//! `DispatchError`) can land independently of the dispatcher loop itself.
//! The dispatcher implementation + retry helpers + idempotency-key
//! derivation land in follow-up PRs.

use crate::push::authentication_header::AuthenticationHeaderBuildError;
use crate::push::nats_push_subject::NatsPushSubject;
use crate::push::push_notification_config_id::PushNotificationConfigIdError;
use crate::push::push_notification_target::PushNotificationTargetError;
use crate::push::target::{WebhookUrl, WebhookUrlError};

#[derive(Clone, Debug, thiserror::Error)]
pub enum DispatchPrepError {
    #[error("{0}")]
    PushConfigId(#[source] PushNotificationConfigIdError),
}

#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("{0}")]
    Prep(#[source] DispatchPrepError),
    #[error("invalid push notification URL: {0}")]
    InvalidTarget(#[source] PushNotificationTargetError),
    #[error("invalid push notification authorization: {0}")]
    InvalidAuthorization(#[source] AuthenticationHeaderBuildError),
    #[error("invalid push notification outbound header value: {0}")]
    InvalidHeader(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// Boxed transport error from the webhook HTTP client. The dispatcher
    /// boxes the concrete `reqwest::Error` into this variant so this
    /// module can carry every dispatch-error shape without taking a direct
    /// dependency on the HTTP client crate.
    #[error("HTTP push request failed: {0}")]
    Http(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("push notification to {url} returned status {status}")]
    UnexpectedStatus { status: u16, url: WebhookUrl },
    #[error(transparent)]
    NatsPublish(NatsPublishDispatchError),
    #[error(transparent)]
    JetStreamPublish(JetStreamPublishDispatchError),
}

#[derive(Debug, thiserror::Error)]
#[error("NATS publish to {subject} failed: {source}")]
pub struct NatsPublishDispatchError {
    subject: NatsPushSubject,
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
}

#[derive(Debug, thiserror::Error)]
#[error("JetStream publish to {subject} failed: {source}")]
pub struct JetStreamPublishDispatchError {
    subject: NatsPushSubject,
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
}

impl NatsPublishDispatchError {
    pub fn new(subject: NatsPushSubject, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            subject,
            source: Box::new(source),
        }
    }

    pub fn subject(&self) -> &NatsPushSubject {
        &self.subject
    }
}

impl JetStreamPublishDispatchError {
    pub fn new(subject: NatsPushSubject, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            subject,
            source: Box::new(source),
        }
    }

    pub fn subject(&self) -> &NatsPushSubject {
        &self.subject
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

#[cfg(test)]
mod tests;
