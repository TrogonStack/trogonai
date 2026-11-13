use trogon_nats::jetstream::StreamMaxAge;
use trogon_nats::{DottedNatsToken, NatsToken};
use trogon_std::NonZeroDuration;

use super::IncidentioSigningSecret;

#[derive(Clone)]
pub struct IncidentioConfig {
    pub signing_secret: IncidentioSigningSecret,
    pub subject_prefix: DottedNatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: StreamMaxAge,
    pub nats_ack_timeout: NonZeroDuration,
    pub timestamp_tolerance: NonZeroDuration,
}

#[cfg(test)]
mod tests;
