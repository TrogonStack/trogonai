use trogon_nats::NatsToken;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_std::NonZeroDuration;

use super::{PlaidApiBaseUrl, PlaidClientId, PlaidSecret};

#[derive(Clone)]
pub struct PlaidConfig {
    pub client_id: PlaidClientId,
    pub secret: PlaidSecret,
    pub api_base_url: PlaidApiBaseUrl,
    pub subject_prefix: NatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: StreamMaxAge,
    pub nats_ack_timeout: NonZeroDuration,
    pub timestamp_tolerance: NonZeroDuration,
}

#[cfg(test)]
mod tests;
