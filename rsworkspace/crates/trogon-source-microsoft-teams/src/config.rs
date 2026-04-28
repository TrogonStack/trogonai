use trogon_nats::NatsToken;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_std::NonZeroDuration;

use crate::client_state::MicrosoftTeamsClientState;

pub struct MicrosoftTeamsConfig {
    pub client_state: MicrosoftTeamsClientState,
    pub subject_prefix: NatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: StreamMaxAge,
    pub nats_ack_timeout: NonZeroDuration,
}
