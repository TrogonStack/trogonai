use super::datadog_webhook_token::DatadogWebhookToken;
use axum::http::HeaderName;
use trogon_nats::NatsToken;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_std::NonZeroDuration;

pub struct DatadogConfig {
    pub webhook_token: DatadogWebhookToken,
    /// HTTP header the shared secret is delivered in. Defaults to
    /// `X-Datadog-Webhook-Token`; operators may override the name.
    pub webhook_token_header: HeaderName,
    pub subject_prefix: NatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: StreamMaxAge,
    pub nats_ack_timeout: NonZeroDuration,
    /// Optional freshness window for the payload `timestamp` field. `None`
    /// disables the check. Datadog does not send a timestamp header, so the
    /// operator must template `$DATE_EPOCH` (POSIX seconds) into the payload.
    pub timestamp_tolerance: Option<NonZeroDuration>,
}
