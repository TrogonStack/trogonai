use async_nats::jetstream::{
    Context,
    consumer::{Consumer, pull},
};
use slack_types::subjects::{
    SLACK_INBOUND, SLACK_INBOUND_BLOCK_ACTION, SLACK_INBOUND_CHANNEL, SLACK_INBOUND_MEMBER,
    SLACK_INBOUND_MESSAGE_CHANGED, SLACK_INBOUND_MESSAGE_DELETED, SLACK_INBOUND_REACTION,
    SLACK_INBOUND_SLASH_COMMAND, SLACK_INBOUND_THREAD_BROADCAST, SLACK_OUTBOUND,
    SLACK_OUTBOUND_REACTION, SLACK_OUTBOUND_STREAM_APPEND, SLACK_OUTBOUND_STREAM_STOP,
};

use crate::setup::STREAM_NAME;

async fn make_consumer(
    js: &Context,
    durable_name: &str,
    filter_subject: &str,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    let stream = js.get_stream(STREAM_NAME).await?;
    let consumer = stream
        .get_or_create_consumer(
            durable_name,
            pull::Config {
                durable_name: Some(durable_name.to_string()),
                filter_subject: filter_subject.to_string(),
                ..Default::default()
            },
        )
        .await?;
    Ok(consumer)
}

// ── Outbound consumers (used by slack-bot) ───────────────────────────────────

pub async fn create_outbound_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-bot-outbound", SLACK_OUTBOUND).await
}

pub async fn create_stream_append_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-bot-stream-append", SLACK_OUTBOUND_STREAM_APPEND).await
}

pub async fn create_stream_stop_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-bot-stream-stop", SLACK_OUTBOUND_STREAM_STOP).await
}

pub async fn create_reaction_action_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-bot-reaction-action", SLACK_OUTBOUND_REACTION).await
}

// ── Inbound consumers (used by slack-agent) ──────────────────────────────────

pub async fn create_inbound_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-agent-inbound", SLACK_INBOUND).await
}

pub async fn create_reaction_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-agent-reaction", SLACK_INBOUND_REACTION).await
}

pub async fn create_message_changed_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        "slack-agent-message-changed",
        SLACK_INBOUND_MESSAGE_CHANGED,
    )
    .await
}

pub async fn create_message_deleted_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        "slack-agent-message-deleted",
        SLACK_INBOUND_MESSAGE_DELETED,
    )
    .await
}

pub async fn create_slash_command_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-agent-slash-command", SLACK_INBOUND_SLASH_COMMAND).await
}

pub async fn create_block_action_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-agent-block-action", SLACK_INBOUND_BLOCK_ACTION).await
}

pub async fn create_thread_broadcast_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        "slack-agent-thread-broadcast",
        SLACK_INBOUND_THREAD_BROADCAST,
    )
    .await
}

pub async fn create_member_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-agent-member", SLACK_INBOUND_MEMBER).await
}

pub async fn create_channel_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-agent-channel", SLACK_INBOUND_CHANNEL).await
}
