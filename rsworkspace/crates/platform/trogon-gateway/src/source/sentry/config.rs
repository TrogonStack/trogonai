use super::sentry_client_secret::SentryClientSecret;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_nats::{DottedNatsToken, NatsToken};
use trogon_std::NonZeroDuration;

pub struct SentryConfig {
    pub client_secret: SentryClientSecret,
    pub subject_prefix: DottedNatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: StreamMaxAge,
    pub nats_ack_timeout: NonZeroDuration,
}
