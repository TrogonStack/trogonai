use trogon_nats::NatsToken;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_std::NonZeroDuration;

use super::GitLabSigningToken;

pub struct GitlabConfig {
    pub signing_token: GitLabSigningToken,
    pub subject_prefix: NatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: StreamMaxAge,
    pub nats_ack_timeout: NonZeroDuration,
    pub timestamp_tolerance: NonZeroDuration,
}
