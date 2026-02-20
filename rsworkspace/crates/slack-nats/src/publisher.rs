use async_nats::jetstream::Context;
use serde::Serialize;
use slack_types::events::{
    SlackAppHomeOpenedEvent, SlackBlockActionEvent, SlackChannelEvent, SlackDeleteFile,
    SlackDeleteMessage, SlackEphemeralMessage, SlackInboundMessage, SlackMemberEvent,
    SlackMessageChangedEvent, SlackMessageDeletedEvent, SlackOutboundMessage, SlackPinEvent,
    SlackProactiveMessage, SlackReactionAction, SlackReactionEvent, SlackReadMessagesRequest,
    SlackReadMessagesResponse, SlackReadRepliesRequest, SlackReadRepliesResponse,
    SlackSetStatusRequest, SlackSetSuggestedPromptsRequest, SlackSlashCommandEvent,
    SlackStreamAppendMessage, SlackStreamStopMessage, SlackThreadBroadcastEvent,
    SlackUpdateMessage, SlackUploadRequest, SlackViewClosedEvent, SlackViewOpenRequest,
    SlackViewPublishRequest, SlackViewSubmissionEvent,
};
use slack_types::subjects::{
    for_account, SLACK_INBOUND, SLACK_INBOUND_APP_HOME, SLACK_INBOUND_BLOCK_ACTION,
    SLACK_INBOUND_CHANNEL, SLACK_INBOUND_MEMBER, SLACK_INBOUND_MESSAGE_CHANGED,
    SLACK_INBOUND_MESSAGE_DELETED, SLACK_INBOUND_PIN, SLACK_INBOUND_REACTION,
    SLACK_INBOUND_SLASH_COMMAND, SLACK_INBOUND_THREAD_BROADCAST, SLACK_INBOUND_VIEW_CLOSED,
    SLACK_INBOUND_VIEW_SUBMISSION, SLACK_OUTBOUND, SLACK_OUTBOUND_DELETE,
    SLACK_OUTBOUND_DELETE_FILE, SLACK_OUTBOUND_EPHEMERAL, SLACK_OUTBOUND_LIST_CONVERSATIONS,
    SLACK_OUTBOUND_LIST_USERS, SLACK_OUTBOUND_PROACTIVE, SLACK_OUTBOUND_REACTION,
    SLACK_OUTBOUND_READ_MESSAGES, SLACK_OUTBOUND_READ_REPLIES, SLACK_OUTBOUND_SET_STATUS,
    SLACK_OUTBOUND_SET_SUGGESTED_PROMPTS, SLACK_OUTBOUND_STREAM_APPEND,
    SLACK_OUTBOUND_STREAM_STOP, SLACK_OUTBOUND_UPDATE, SLACK_OUTBOUND_UPLOAD,
    SLACK_OUTBOUND_VIEW_OPEN, SLACK_OUTBOUND_VIEW_PUBLISH,
};

async fn js_publish<T: Serialize>(
    js: &Context,
    subject: &str,
    msg: &T,
) -> Result<(), async_nats::Error> {
    let payload = serde_json::to_vec(msg)?;
    js.publish(subject.to_string(), payload.into()).await?.await?;
    Ok(())
}

pub async fn publish_inbound(
    js: &Context,
    account_id: Option<&str>,
    msg: &SlackInboundMessage,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_INBOUND, account_id), msg).await
}

pub async fn publish_reaction(
    js: &Context,
    account_id: Option<&str>,
    ev: &SlackReactionEvent,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_INBOUND_REACTION, account_id), ev).await
}

pub async fn publish_message_changed(
    js: &Context,
    account_id: Option<&str>,
    ev: &SlackMessageChangedEvent,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_INBOUND_MESSAGE_CHANGED, account_id), ev).await
}

pub async fn publish_message_deleted(
    js: &Context,
    account_id: Option<&str>,
    ev: &SlackMessageDeletedEvent,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_INBOUND_MESSAGE_DELETED, account_id), ev).await
}

pub async fn publish_thread_broadcast(
    js: &Context,
    account_id: Option<&str>,
    ev: &SlackThreadBroadcastEvent,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_INBOUND_THREAD_BROADCAST, account_id), ev).await
}

pub async fn publish_member(
    js: &Context,
    account_id: Option<&str>,
    ev: &SlackMemberEvent,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_INBOUND_MEMBER, account_id), ev).await
}

pub async fn publish_channel(
    js: &Context,
    account_id: Option<&str>,
    ev: &SlackChannelEvent,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_INBOUND_CHANNEL, account_id), ev).await
}

pub async fn publish_slash_command(
    js: &Context,
    account_id: Option<&str>,
    ev: &SlackSlashCommandEvent,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_INBOUND_SLASH_COMMAND, account_id), ev).await
}

// ── Outbound publishers (agent → NATS → slack-bot → Slack) ──────────────────

/// Publish a one-shot outbound message (`chat.postMessage`).
pub async fn publish_outbound(
    js: &Context,
    account_id: Option<&str>,
    msg: &SlackOutboundMessage,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_OUTBOUND, account_id), msg).await
}

/// Publish an in-flight streaming update (`chat.update` — append step).
pub async fn publish_stream_append(
    js: &Context,
    account_id: Option<&str>,
    msg: &SlackStreamAppendMessage,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_OUTBOUND_STREAM_APPEND, account_id), msg).await
}

/// Publish the final streaming update (`chat.update` — stop step).
pub async fn publish_stream_stop(
    js: &Context,
    account_id: Option<&str>,
    msg: &SlackStreamStopMessage,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_OUTBOUND_STREAM_STOP, account_id), msg).await
}

pub async fn publish_block_action(
    js: &Context,
    account_id: Option<&str>,
    ev: &SlackBlockActionEvent,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_INBOUND_BLOCK_ACTION, account_id), ev).await
}

/// Publish a reaction action (add or remove an emoji on a message).
pub async fn publish_reaction_action(
    js: &Context,
    account_id: Option<&str>,
    action: &SlackReactionAction,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_OUTBOUND_REACTION, account_id), action).await
}

pub async fn publish_app_home(
    js: &Context,
    account_id: Option<&str>,
    ev: &SlackAppHomeOpenedEvent,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_INBOUND_APP_HOME, account_id), ev).await
}

pub async fn publish_view_submission(
    js: &Context,
    account_id: Option<&str>,
    ev: &SlackViewSubmissionEvent,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_INBOUND_VIEW_SUBMISSION, account_id), ev).await
}

/// Publish a request to open a modal (`views.open`).
pub async fn publish_view_open(
    js: &Context,
    account_id: Option<&str>,
    req: &SlackViewOpenRequest,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_OUTBOUND_VIEW_OPEN, account_id), req).await
}

/// Publish a request to update the App Home view (`views.publish`).
pub async fn publish_view_publish(
    js: &Context,
    account_id: Option<&str>,
    req: &SlackViewPublishRequest,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_OUTBOUND_VIEW_PUBLISH, account_id), req).await
}

pub async fn publish_view_closed(
    js: &Context,
    account_id: Option<&str>,
    event: &SlackViewClosedEvent,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_INBOUND_VIEW_CLOSED, account_id), event).await
}

pub async fn publish_pin(
    js: &Context,
    account_id: Option<&str>,
    event: &SlackPinEvent,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_INBOUND_PIN, account_id), event).await
}

pub async fn publish_set_status(
    js: &Context,
    account_id: Option<&str>,
    req: &SlackSetStatusRequest,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_OUTBOUND_SET_STATUS, account_id), req).await
}

pub async fn publish_delete_message(
    js: &Context,
    account_id: Option<&str>,
    msg: &SlackDeleteMessage,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_OUTBOUND_DELETE, account_id), msg).await
}

pub async fn publish_update_message(
    js: &Context,
    account_id: Option<&str>,
    msg: &SlackUpdateMessage,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_OUTBOUND_UPDATE, account_id), msg).await
}

/// Fetch channel history from the bot via Core NATS request/reply.
pub async fn request_read_messages(
    client: &async_nats::Client,
    account_id: Option<&str>,
    req: &SlackReadMessagesRequest,
) -> Result<SlackReadMessagesResponse, async_nats::Error> {
    let subject = for_account(SLACK_OUTBOUND_READ_MESSAGES, account_id);
    let payload = serde_json::to_vec(req)?;
    let response = client.request(subject, payload.into()).await?;
    let result: SlackReadMessagesResponse = serde_json::from_slice(&response.payload)?;
    Ok(result)
}

/// Publish a request to upload content as a Slack file.
pub async fn publish_upload_request(
    js: &Context,
    account_id: Option<&str>,
    req: &SlackUploadRequest,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_OUTBOUND_UPLOAD, account_id), req).await
}

/// Fetch thread replies from the bot via Core NATS request/reply.
pub async fn request_read_replies(
    client: &async_nats::Client,
    account_id: Option<&str>,
    req: &SlackReadRepliesRequest,
) -> Result<SlackReadRepliesResponse, async_nats::Error> {
    let subject = for_account(SLACK_OUTBOUND_READ_REPLIES, account_id);
    let payload = serde_json::to_vec(req)?;
    let response = client.request(subject, payload.into()).await?;
    let result: SlackReadRepliesResponse = serde_json::from_slice(&response.payload)?;
    Ok(result)
}

/// Publish a request to set suggested prompts on an assistant thread.
pub async fn publish_set_suggested_prompts(
    js: &Context,
    account_id: Option<&str>,
    req: &SlackSetSuggestedPromptsRequest,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_OUTBOUND_SET_SUGGESTED_PROMPTS, account_id), req).await
}

/// Publish a proactive message (no inbound trigger required).
pub async fn publish_proactive_message(
    js: &Context,
    account_id: Option<&str>,
    msg: &SlackProactiveMessage,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_OUTBOUND_PROACTIVE, account_id), msg).await
}

/// Publish an ephemeral message (visible only to a specific user).
pub async fn publish_ephemeral_message(
    js: &Context,
    account_id: Option<&str>,
    msg: &SlackEphemeralMessage,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_OUTBOUND_EPHEMERAL, account_id), msg).await
}

/// Publish a request to delete an uploaded file.
pub async fn publish_delete_file(
    js: &Context,
    account_id: Option<&str>,
    req: &SlackDeleteFile,
) -> Result<(), async_nats::Error> {
    js_publish(js, &for_account(SLACK_OUTBOUND_DELETE_FILE, account_id), req).await
}

/// List workspace users from the bot via Core NATS request/reply.
pub async fn request_list_users(
    client: &async_nats::Client,
    account_id: Option<&str>,
    req: &slack_types::events::SlackListUsersRequest,
) -> Result<slack_types::events::SlackListUsersResponse, String> {
    let subject = for_account(SLACK_OUTBOUND_LIST_USERS, account_id);
    let payload = serde_json::to_vec(req).map_err(|e| e.to_string())?;
    let response = client.request(subject, payload.into())
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&response.payload).map_err(|e| e.to_string())
}

/// List channels/conversations from the bot via Core NATS request/reply.
pub async fn request_list_conversations(
    client: &async_nats::Client,
    account_id: Option<&str>,
    req: &slack_types::events::SlackListConversationsRequest,
) -> Result<slack_types::events::SlackListConversationsResponse, String> {
    let subject = for_account(SLACK_OUTBOUND_LIST_CONVERSATIONS, account_id);
    let payload = serde_json::to_vec(req).map_err(|e| e.to_string())?;
    let response = client.request(subject, payload.into())
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&response.payload).map_err(|e| e.to_string())
}
