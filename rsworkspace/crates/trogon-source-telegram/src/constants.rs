use trogon_std::{ByteSize, HttpBodySizeMax};

pub const HTTP_BODY_SIZE_MAX: HttpBodySizeMax = HttpBodySizeMax::new(ByteSize::mib(10)).unwrap();

pub const HEADER_SECRET_TOKEN: &str = "x-telegram-bot-api-secret-token";

pub const NATS_HEADER_UPDATE_TYPE: &str = "X-Telegram-Update-Type";
pub const NATS_HEADER_UPDATE_ID: &str = "X-Telegram-Update-Id";

pub const UPDATE_TYPES: &[&str] = &[
    "message",
    "edited_message",
    "channel_post",
    "edited_channel_post",
    "business_connection",
    "business_message",
    "edited_business_message",
    "deleted_business_messages",
    "message_reaction",
    "message_reaction_count",
    "inline_query",
    "chosen_inline_result",
    "callback_query",
    "shipping_query",
    "pre_checkout_query",
    "purchased_paid_media",
    "poll",
    "poll_answer",
    "my_chat_member",
    "chat_member",
    "chat_join_request",
    "chat_boost",
    "removed_chat_boost",
    "managed_bot",
];
