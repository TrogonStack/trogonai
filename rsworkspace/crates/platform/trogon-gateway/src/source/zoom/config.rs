use trogon_nats::jetstream::StreamMaxAge;
use trogon_nats::{DottedNatsToken, NatsToken};
use trogon_std::NonZeroDuration;

use super::ZoomSecretToken;

#[derive(Clone)]
pub struct ZoomConfig {
    pub secret_token: ZoomSecretToken,
    pub subject_prefix: DottedNatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: StreamMaxAge,
    pub nats_ack_timeout: NonZeroDuration,
    pub timestamp_tolerance: NonZeroDuration,
}

#[cfg(test)]
mod tests;
