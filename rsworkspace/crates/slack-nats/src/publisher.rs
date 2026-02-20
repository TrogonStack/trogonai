use async_nats::jetstream::Context;
use serde::Serialize;
use slack_types::events::{
    SlackAppHomeOpenedEvent, SlackBlockActionEvent, SlackChannelEvent, SlackInboundMessage,
    SlackMemberEvent, SlackMessageChangedEvent, SlackMessageDeletedEvent, SlackOutboundMessage,
    SlackReactionAction, SlackReactionEvent, SlackSlashCommandEvent, SlackStreamAppendMessage,
    SlackStreamStopMessage, SlackThreadBroadcastEvent, SlackViewOpenRequest,
    SlackViewPublishRequest, SlackViewSubmissionEvent,
};
use slack_types::subjects::{
    SLACK_INBOUND, SLACK_INBOUND_APP_HOME, SLACK_INBOUND_BLOCK_ACTION, SLACK_INBOUND_CHANNEL,
    SLACK_INBOUND_MEMBER, SLACK_INBOUND_MESSAGE_CHANGED, SLACK_INBOUND_MESSAGE_DELETED,
    SLACK_INBOUND_REACTION, SLACK_INBOUND_SLASH_COMMAND, SLACK_INBOUND_THREAD_BROADCAST,
    SLACK_INBOUND_VIEW_SUBMISSION, SLACK_OUTBOUND, SLACK_OUTBOUND_REACTION,
    SLACK_OUTBOUND_STREAM_APPEND, SLACK_OUTBOUND_STREAM_STOP, SLACK_OUTBOUND_VIEW_OPEN,
    SLACK_OUTBOUND_VIEW_PUBLISH,
};

async fn js_publish<T: Serialize>(
    js: &Context,
    subject: &'static str,
    msg: &T,
) -> Result<(), async_nats::Error> {
    let payload = serde_json::to_vec(msg)?;
    js.publish(subject, payload.into()).await?.await?;
    Ok(())
}

pub async fn publish_inbound(
    js: &Context,
    msg: &SlackInboundMessage,
) -> Result<(), async_nats::Error> {
    js_publish(js, SLACK_INBOUND, msg).await
}

pub async fn publish_reaction(
    js: &Context,
    ev: &SlackReactionEvent,
) -> Result<(), async_nats::Error> {
    js_publish(js, SLACK_INBOUND_REACTION, ev).await
}

pub async fn publish_message_changed(
    js: &Context,
    ev: &SlackMessageChangedEvent,
) -> Result<(), async_nats::Error> {
    js_publish(js, SLACK_INBOUND_MESSAGE_CHANGED, ev).await
}

pub async fn publish_message_deleted(
    js: &Context,
    ev: &SlackMessageDeletedEvent,
) -> Result<(), async_nats::Error> {
    js_publish(js, SLACK_INBOUND_MESSAGE_DELETED, ev).await
}

pub async fn publish_thread_broadcast(
    js: &Context,
    ev: &SlackThreadBroadcastEvent,
) -> Result<(), async_nats::Error> {
    js_publish(js, SLACK_INBOUND_THREAD_BROADCAST, ev).await
}

pub async fn publish_member(js: &Context, ev: &SlackMemberEvent) -> Result<(), async_nats::Error> {
    js_publish(js, SLACK_INBOUND_MEMBER, ev).await
}

pub async fn publish_channel(
    js: &Context,
    ev: &SlackChannelEvent,
) -> Result<(), async_nats::Error> {
    js_publish(js, SLACK_INBOUND_CHANNEL, ev).await
}

pub async fn publish_slash_command(
    js: &Context,
    ev: &SlackSlashCommandEvent,
) -> Result<(), async_nats::Error> {
    js_publish(js, SLACK_INBOUND_SLASH_COMMAND, ev).await
}

// ── Outbound publishers (agent → NATS → slack-bot → Slack) ──────────────────

/// Publish a one-shot outbound message (`chat.postMessage`).
pub async fn publish_outbound(
    js: &Context,
    msg: &SlackOutboundMessage,
) -> Result<(), async_nats::Error> {
    js_publish(js, SLACK_OUTBOUND, msg).await
}

/// Publish an in-flight streaming update (`chat.update` — append step).
pub async fn publish_stream_append(
    js: &Context,
    msg: &SlackStreamAppendMessage,
) -> Result<(), async_nats::Error> {
    js_publish(js, SLACK_OUTBOUND_STREAM_APPEND, msg).await
}

/// Publish the final streaming update (`chat.update` — stop step).
pub async fn publish_stream_stop(
    js: &Context,
    msg: &SlackStreamStopMessage,
) -> Result<(), async_nats::Error> {
    js_publish(js, SLACK_OUTBOUND_STREAM_STOP, msg).await
}

pub async fn publish_block_action(
    js: &Context,
    ev: &SlackBlockActionEvent,
) -> Result<(), async_nats::Error> {
    js_publish(js, SLACK_INBOUND_BLOCK_ACTION, ev).await
}

/// Publish a reaction action (add or remove an emoji on a message).
pub async fn publish_reaction_action(
    js: &Context,
    action: &SlackReactionAction,
) -> Result<(), async_nats::Error> {
    js_publish(js, SLACK_OUTBOUND_REACTION, action).await
}

pub async fn publish_app_home(
    js: &Context,
    ev: &SlackAppHomeOpenedEvent,
) -> Result<(), async_nats::Error> {
    js_publish(js, SLACK_INBOUND_APP_HOME, ev).await
}

pub async fn publish_view_submission(
    js: &Context,
    ev: &SlackViewSubmissionEvent,
) -> Result<(), async_nats::Error> {
    js_publish(js, SLACK_INBOUND_VIEW_SUBMISSION, ev).await
}

/// Publish a request to open a modal (`views.open`).
pub async fn publish_view_open(
    js: &Context,
    req: &SlackViewOpenRequest,
) -> Result<(), async_nats::Error> {
    js_publish(js, SLACK_OUTBOUND_VIEW_OPEN, req).await
}

/// Publish a request to update the App Home view (`views.publish`).
pub async fn publish_view_publish(
    js: &Context,
    req: &SlackViewPublishRequest,
) -> Result<(), async_nats::Error> {
    js_publish(js, SLACK_OUTBOUND_VIEW_PUBLISH, req).await
}
