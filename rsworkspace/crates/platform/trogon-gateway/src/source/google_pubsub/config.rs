use trogon_nats::NatsToken;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_std::NonZeroDuration;

use super::credentials::GoogleCredentials;
use super::max_outstanding_messages::MaxOutstandingMessages;
use super::pub_sub_subscription_name::PubSubSubscriptionName;

#[derive(Clone, Debug)]
pub struct StreamingPullConfig {
    pub subscription: PubSubSubscriptionName,
    pub max_outstanding_messages: Option<MaxOutstandingMessages>,
    pub credentials: GoogleCredentials,
}

#[derive(Clone, Debug)]
pub enum GooglePubSubTransportConfig {
    StreamingPull(StreamingPullConfig),
}

#[derive(Clone, Debug)]
pub struct GooglePubSubConfig {
    pub subject_prefix: NatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: StreamMaxAge,
    pub nats_ack_timeout: NonZeroDuration,
    pub transport: GooglePubSubTransportConfig,
}
