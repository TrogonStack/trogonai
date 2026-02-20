use async_nats::jetstream::{
    Context,
    consumer::{Consumer, pull},
};
use slack_types::subjects::{
    SLACK_INBOUND, SLACK_INBOUND_APP_HOME, SLACK_INBOUND_BLOCK_ACTION, SLACK_INBOUND_CHANNEL,
    SLACK_INBOUND_MEMBER, SLACK_INBOUND_MESSAGE_CHANGED, SLACK_INBOUND_MESSAGE_DELETED,
    SLACK_INBOUND_PIN, SLACK_INBOUND_REACTION, SLACK_INBOUND_SLASH_COMMAND,
    SLACK_INBOUND_THREAD_BROADCAST, SLACK_INBOUND_VIEW_CLOSED, SLACK_INBOUND_VIEW_SUBMISSION,
    SLACK_OUTBOUND, SLACK_OUTBOUND_DELETE, SLACK_OUTBOUND_DELETE_FILE, SLACK_OUTBOUND_EPHEMERAL,
    SLACK_OUTBOUND_PROACTIVE, SLACK_OUTBOUND_REACTION, SLACK_OUTBOUND_SET_STATUS,
    SLACK_OUTBOUND_SET_SUGGESTED_PROMPTS, SLACK_OUTBOUND_STREAM_APPEND,
    SLACK_OUTBOUND_STREAM_STOP, SLACK_OUTBOUND_UPDATE, SLACK_OUTBOUND_UPLOAD,
    SLACK_OUTBOUND_VIEW_OPEN, SLACK_OUTBOUND_VIEW_PUBLISH,
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

pub async fn create_app_home_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-agent-app-home", SLACK_INBOUND_APP_HOME).await
}

pub async fn create_view_submission_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-agent-view-submission", SLACK_INBOUND_VIEW_SUBMISSION).await
}

// ── Bot consumers for view outbound ──────────────────────────────────────────

pub async fn create_view_open_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-bot-view-open", SLACK_OUTBOUND_VIEW_OPEN).await
}

pub async fn create_view_publish_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-bot-view-publish", SLACK_OUTBOUND_VIEW_PUBLISH).await
}

pub async fn create_view_closed_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-agent-view-closed", SLACK_INBOUND_VIEW_CLOSED).await
}

pub async fn create_pin_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-agent-pin", SLACK_INBOUND_PIN).await
}

pub async fn create_set_status_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-bot-set-status", SLACK_OUTBOUND_SET_STATUS).await
}

pub async fn create_delete_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-bot-delete", SLACK_OUTBOUND_DELETE).await
}

pub async fn create_update_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-bot-update", SLACK_OUTBOUND_UPDATE).await
}

pub async fn create_upload_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-bot-upload", SLACK_OUTBOUND_UPLOAD).await
}

pub async fn create_suggested_prompts_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        "slack-bot-suggested-prompts",
        SLACK_OUTBOUND_SET_SUGGESTED_PROMPTS,
    )
    .await
}

pub async fn create_proactive_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-bot-proactive", SLACK_OUTBOUND_PROACTIVE).await
}

pub async fn create_ephemeral_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-bot-ephemeral", SLACK_OUTBOUND_EPHEMERAL).await
}

pub async fn create_delete_file_consumer(
    js: &Context,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(js, "slack-bot-delete-file", SLACK_OUTBOUND_DELETE_FILE).await
}
