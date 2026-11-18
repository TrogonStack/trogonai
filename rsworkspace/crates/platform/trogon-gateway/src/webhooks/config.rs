use std::time::Duration;

use trogon_nats::jetstream::StreamMaxAge;
use trogon_nats::{DottedNatsToken, NatsToken, SubjectTokenViolationError};
use trogon_std::{NonZeroDuration, ZeroDurationError};

use crate::source_status::{SourceStatus, SourceStatusError};

use super::registry_bucket::{RegistryBucket, RegistryBucketError};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfigInput {
    status: Option<String>,
    registry_bucket: Option<String>,
    stream_name: Option<String>,
    subject_prefix: Option<String>,
    stream_max_age_secs: Option<u64>,
    duplicate_window_secs: Option<u64>,
    admission_timeout_secs: Option<u64>,
}

#[derive(Clone)]
pub struct WebhookConfig {
    pub registry_bucket: RegistryBucket,
    pub stream_name: NatsToken,
    pub subject_prefix: DottedNatsToken,
    pub stream_max_age: StreamMaxAge,
    pub duplicate_window: NonZeroDuration,
    pub admission_timeout: NonZeroDuration,
}

#[derive(Debug, thiserror::Error)]
pub enum WebhookConfigError {
    #[error(transparent)]
    Status(#[from] SourceStatusError),
    #[error(transparent)]
    RegistryBucket(#[from] RegistryBucketError),
    #[error("invalid {field}: {source}")]
    Subject {
        field: &'static str,
        #[source]
        source: SubjectTokenViolationError,
    },
    #[error("{field} must be greater than zero")]
    ZeroDuration {
        field: &'static str,
        #[source]
        source: ZeroDurationError,
    },
    #[error("duplicate window must not exceed seven days or the stream retention")]
    DuplicateWindow,
}

impl WebhookConfigInput {
    pub fn resolve(self) -> Result<Option<WebhookConfig>, WebhookConfigError> {
        let status = self.status.as_deref().unwrap_or("enabled").parse::<SourceStatus>()?;
        if status == SourceStatus::Disabled {
            return Ok(None);
        }
        let registry_bucket = RegistryBucket::new(
            self.registry_bucket
                .unwrap_or_else(|| "GATEWAY_WEBHOOK_ENDPOINTS".to_owned()),
        )?;
        let stream_name =
            NatsToken::new(self.stream_name.unwrap_or_else(|| "WEBHOOKS".to_owned())).map_err(|source| {
                WebhookConfigError::Subject {
                    field: "stream_name",
                    source,
                }
            })?;
        let subject_prefix = DottedNatsToken::new(self.subject_prefix.unwrap_or_else(|| "webhooks".to_owned()))
            .map_err(|source| WebhookConfigError::Subject {
                field: "subject_prefix",
                source,
            })?;
        let stream_max_age =
            StreamMaxAge::from_secs(self.stream_max_age_secs.unwrap_or(604_800)).map_err(|source| {
                WebhookConfigError::ZeroDuration {
                    field: "stream_max_age_secs",
                    source,
                }
            })?;
        let duplicate_window =
            NonZeroDuration::from_secs(self.duplicate_window_secs.unwrap_or(604_800)).map_err(|source| {
                WebhookConfigError::ZeroDuration {
                    field: "duplicate_window_secs",
                    source,
                }
            })?;
        let duplicate_duration = Duration::from(duplicate_window);
        if duplicate_duration > Duration::from_secs(604_800)
            || matches!(stream_max_age, StreamMaxAge::ExpireAfter(age) if duplicate_duration > Duration::from(age))
        {
            return Err(WebhookConfigError::DuplicateWindow);
        }
        let admission_timeout =
            NonZeroDuration::from_secs(self.admission_timeout_secs.unwrap_or(10)).map_err(|source| {
                WebhookConfigError::ZeroDuration {
                    field: "admission_timeout_secs",
                    source,
                }
            })?;
        Ok(Some(WebhookConfig {
            registry_bucket,
            stream_name,
            subject_prefix,
            stream_max_age,
            duplicate_window,
            admission_timeout,
        }))
    }
}
