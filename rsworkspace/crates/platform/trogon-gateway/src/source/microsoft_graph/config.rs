use trogon_nats::jetstream::StreamMaxAge;
use trogon_nats::{DottedNatsToken, NatsToken};
use trogon_std::NonZeroDuration;

use super::client_state::MicrosoftGraphClientState;

pub struct MicrosoftGraphConfig {
    pub client_state: MicrosoftGraphClientState,
    pub subject_prefix: DottedNatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: StreamMaxAge,
    pub nats_ack_timeout: NonZeroDuration,
}
