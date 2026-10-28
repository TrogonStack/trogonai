use std::time::Duration;

pub const SOURCE_NAME: &str = "google-pubsub";

pub const SUBSCRIPTION_ID_MIN_LEN: usize = 3;
pub const SUBSCRIPTION_ID_MAX_LEN: usize = 255;
pub const SUBSCRIPTION_ID_RESERVED_PREFIX: &str = "goog";

pub const SUBJECT_SUFFIX_MESSAGE: &str = "message";

pub const NATS_HEADER_MESSAGE_ID: &str = "X-PubSub-Message-Id";
pub const NATS_HEADER_PUBLISH_TIME: &str = "X-PubSub-Publish-Time";
pub const NATS_HEADER_ORDERING_KEY: &str = "X-PubSub-Ordering-Key";
pub const NATS_HEADER_SUBSCRIPTION: &str = "X-PubSub-Subscription";
pub const NATS_HEADER_ATTRIBUTE_PREFIX: &str = "X-PubSub-Attribute-";

pub const RESUBSCRIBE_INITIAL_DELAY: Duration = Duration::from_secs(1);
pub const RESUBSCRIBE_MAX_DELAY: Duration = Duration::from_secs(30);

pub const REDELIVERY_PAUSE: Duration = Duration::from_secs(1);
