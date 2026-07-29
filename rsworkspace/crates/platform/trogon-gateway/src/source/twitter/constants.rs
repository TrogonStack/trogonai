use trogon_std::{ByteSize, HttpBodySizeMax};

pub const HTTP_BODY_SIZE_MAX: HttpBodySizeMax = HttpBodySizeMax::new(ByteSize::mib(2)).unwrap();

pub const HEADER_SIGNATURE: &str = "x-twitter-webhooks-signature";

pub const NATS_HEADER_EVENT_TYPE: &str = "X-Twitter-Event-Type";
pub const NATS_HEADER_PAYLOAD_KIND: &str = "X-Twitter-Payload-Kind";
pub const NATS_HEADER_REJECT_REASON: &str = "X-Twitter-Reject-Reason";

pub const ACCOUNT_ACTIVITY_EVENT_TYPES: &[&str] = &[
    "tweet_create_events",
    "favorite_events",
    "follow_events",
    "unfollow_events",
    "block_events",
    "unblock_events",
    "mute_events",
    "unmute_events",
    "user_event",
    "direct_message_events",
    "direct_message_indicate_typing_events",
    "direct_message_mark_read_events",
    "tweet_delete_events",
];
